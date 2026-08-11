#[allow(
    clippy::wildcard_imports,
    reason = "discovery methods operate on the supervisor's private orchestration vocabulary"
)]
use super::*;

impl Supervisor {
    pub(super) fn discover_output(&mut self) -> Discovery<OutputSession> {
        if self.config.output != OutputSelection::Serial {
            return make_nonserial_output(&self.config.output).map_or_else(
                Discovery::Error,
                |output| {
                    self.update_status(|status| {
                        status.xiao = XiaoStatus::default();
                    });
                    Discovery::Ready(OutputSession {
                        output,
                        xiao: None,
                        first_observed_receipt: FirstObservedReceiptState::Idle,
                    })
                },
            );
        }

        let devices = match available_serial_devices() {
            Ok(devices) => devices,
            Err(error) => {
                return Discovery::Wait {
                    detail: "Cannot enumerate serial ports".to_owned(),
                    error: Some(error.to_string()),
                };
            }
        };
        let candidates: Vec<_> = match &self.config.serial {
            SerialSelection::AutoXiao => devices
                .into_iter()
                .filter(SerialDeviceInfo::is_xiao_bridge)
                .collect(),
            SerialSelection::Port(path) => devices
                .into_iter()
                .filter(|device| &device.path == path)
                .collect(),
        };
        if candidates.is_empty() {
            let detail = match &self.config.serial {
                SerialSelection::AutoXiao => {
                    "Waiting for XIAO Steam Controller Bridge CDC port".to_owned()
                }
                SerialSelection::Port(path) => format!("Waiting for XIAO serial port {path}"),
            };
            return Discovery::Wait {
                detail,
                error: None,
            };
        }

        let mut valid = Vec::new();
        let mut failures = Vec::new();
        for candidate in candidates {
            match SerialOutput::open(
                &candidate.path,
                self.config.baud_rate,
                self.config.serial_config,
            ) {
                Ok(output) => valid.push((candidate, output)),
                Err(error) => failures.push(format!("{}: {error}", candidate.path)),
            }
        }
        if valid.is_empty() {
            return Discovery::Wait {
                detail: "Waiting for a XIAO that completes the protocol-v1 Hello handshake"
                    .to_owned(),
                error: (!failures.is_empty()).then(|| failures.join("; ")),
            };
        }

        let selected_index = match choose_xiao_index(&valid, self.preferred_xiao_serial.as_deref())
        {
            Ok(index) => index,
            Err(message) => return Discovery::Error(message),
        };
        let (info, output) = valid.swap_remove(selected_index);
        self.preferred_xiao_serial.clone_from(&info.serial_number);
        // DeviceInfo can land in the same read burst as the HelloResponse
        // during the blocking handshake, so read it rather than assume
        // Pending.
        let firmware = output.firmware_info().unwrap_or_default();
        self.update_status(|status| {
            status.xiao = XiaoStatus {
                path: Some(info.path.clone()),
                usb_serial: info.serial_number.clone(),
                handshake_complete: true,
                firmware,
            };
        });
        eprintln!(
            "level=info event=xiao_ready path={:?} usb_serial={} protocol=1",
            info.path,
            masked_serial(info.serial_number.as_deref())
        );
        Discovery::Ready(OutputSession {
            output: Box::new(output),
            xiao: Some(info),
            first_observed_receipt: FirstObservedReceiptState::Idle,
        })
    }

    pub(super) fn discover_controller_source(&mut self) -> Discovery<ActiveControllerSource> {
        match self.config.controller {
            ControllerSelection::Index(index) => {
                if self.indexed_controller_discovery.scan_due() {
                    let discovered = self
                        .controller_enumerator()
                        .and_then(ControllerEnumerator::enumerate_all)
                        .map_err(|error| error.to_string());
                    self.indexed_controller_discovery.refresh(index, discovered);
                }
                if let Some(error) = self.indexed_controller_discovery.scan_error() {
                    return Discovery::Wait {
                        detail: "Cannot enumerate Steam Controller HID collections".to_owned(),
                        error: Some(error.to_owned()),
                    };
                }
                let Some(info) = self.indexed_controller_discovery.info().cloned() else {
                    return Discovery::Wait {
                        detail: format!("Waiting for Steam Controller collection index {index}"),
                        error: None,
                    };
                };
                if !info.is_supported_controller_source() {
                    return Discovery::Error(format!(
                        "collection index {index} is not a supported Steam Controller 2 input; \
                         expected a 28de:1304 USB Puck ff00:0001 interface 2-5 or the \
                         28de:1303 Bluetooth ff00:0001 interface -1 collection"
                    ));
                }
                if self.source_on_cooldown(&info) {
                    return Discovery::Wait {
                        detail:
                            "Controller is finishing automatic shutdown; waiting for a fresh wake"
                                .to_owned(),
                        error: None,
                    };
                }
                match self
                    .controller_enumerator()
                    .and_then(|enumerator| enumerator.open(&info))
                {
                    Ok(mut session) => {
                        // Consume the synthetic open event here. The worker has
                        // already performed its initial suppression before it
                        // forwards any lifecycle or input event.
                        let _ = session.poll(Duration::ZERO);
                        self.update_source_discovered(&info, false);
                        self.indexed_controller_discovery.clear();
                        Discovery::Ready(ActiveControllerSource {
                            info,
                            session,
                            controller_seen: false,
                        })
                    }
                    Err(error) => Discovery::Wait {
                        detail: format!(
                            "Waiting to open Steam Controller collection index {index}"
                        ),
                        error: Some(ownership_guidance(&error)),
                    },
                }
            }
            ControllerSelection::AutoActive => self.discover_active_controller_source(),
        }
    }

    pub(super) fn discover_active_controller_source(
        &mut self,
    ) -> Discovery<ActiveControllerSource> {
        if self.controller_discovery.scan_due() {
            let discovered = self
                .enumerate_controller_candidates()
                .map_err(|error| error.to_string());
            // Borrowed as a separate field so the open closure can reuse the
            // shared context while the discovery state is mutated.
            let enumerator = self.controller_enumerator.as_ref();
            self.controller_discovery
                .refresh(discovered, |_, info| match enumerator {
                    Some(enumerator) => enumerator.open(info).map_err(|error| {
                        format!(
                            "{}: {}",
                            controller_source_identity(info),
                            ownership_guidance(&error)
                        )
                    }),
                    None => Err("the HID context is unavailable".to_owned()),
                });
        }

        if self.controller_discovery.is_empty() {
            if let Some(error) = self.controller_discovery.scan_error() {
                return Discovery::Wait {
                    detail: "Cannot enumerate Steam Controller HID collections".to_owned(),
                    error: Some(error.to_owned()),
                };
            }
            if self.controller_discovery.supported_devices_seen() {
                return Discovery::Wait {
                    detail: "Steam Controller input found, but no collection can be opened"
                        .to_owned(),
                    error: self.controller_discovery.current_errors(&[]),
                };
            }
            return Discovery::Wait {
                detail: "Waiting for a Steam Controller 2 Puck or Bluetooth connection".to_owned(),
                error: None,
            };
        }

        let probe = self.controller_discovery.probe();
        match choose_unique_active(&probe.active_indices) {
            Ok(None) => Discovery::Wait {
                detail: "Steam Controller input found; waiting for valid controller state"
                    .to_owned(),
                error: self.controller_discovery.current_errors(&probe.failures),
            },
            Ok(Some(selected)) => {
                let selected_info = self.controller_discovery.candidate(selected).info().clone();
                if self.source_on_cooldown(&selected_info) {
                    return Discovery::Wait {
                        detail:
                            "Controller is finishing automatic shutdown; waiting for a fresh wake"
                                .to_owned(),
                        error: None,
                    };
                }
                let (info, session) = self.controller_discovery.select(selected).into_parts();
                self.update_source_discovered(&info, true);
                Discovery::Ready(ActiveControllerSource {
                    info,
                    session,
                    controller_seen: true,
                })
            }
            Err(active_indices) => {
                let global = self
                    .controller_enumerator()
                    .and_then(ControllerEnumerator::enumerate_all);
                let global_indices_available = match global {
                    Ok(devices) => self
                        .controller_discovery
                        .resolve_global_indices(&devices)
                        .is_ok(),
                    Err(_) => false,
                };
                let sources = active_indices
                    .iter()
                    .map(|index| {
                        let candidate = self.controller_discovery.candidate(*index);
                        if global_indices_available {
                            controller_source_description(
                                candidate.enumeration_index(),
                                candidate.info(),
                            )
                        } else {
                            controller_source_identity(candidate.info())
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                Discovery::Error(format!(
                    "multiple active Steam Controller 2 input sources were detected: {sources}; \
                     run sc-probe list and restart with --index N"
                ))
            }
        }
    }

    /// Returns the shared HID context, building it on first use.
    ///
    /// One context serves filtered scans, full-inventory scans, and opening
    /// sessions. Constructing a context enumerates every collection in the
    /// system, so creating one per scan or per open attempt is what made idle
    /// discovery expensive.
    pub(super) fn controller_enumerator(
        &mut self,
    ) -> Result<&mut ControllerEnumerator, DeviceError> {
        if self.controller_enumerator.is_none() {
            self.controller_enumerator = Some(ControllerEnumerator::new()?);
        }
        Ok(self
            .controller_enumerator
            .as_mut()
            .expect("controller enumerator was initialized"))
    }

    pub(super) fn enumerate_controller_candidates(
        &mut self,
    ) -> Result<Vec<(usize, HidDeviceInfo)>, DeviceError> {
        self.controller_enumerator()
            .and_then(ControllerEnumerator::enumerate)
            .map(|devices| devices.into_iter().enumerate().collect())
    }

    pub(super) fn clear_controller_discovery(&mut self) {
        self.controller_discovery.clear();
        self.indexed_controller_discovery.clear();
    }

    pub(super) fn source_on_cooldown(&mut self, info: &HidDeviceInfo) -> bool {
        let now = Instant::now();
        if self
            .controller_cooldown
            .as_ref()
            .is_some_and(|cooldown| now >= cooldown.until)
        {
            self.controller_cooldown = None;
        }
        self.controller_cooldown.as_ref().is_some_and(|cooldown| {
            same_controller_collection(&cooldown.info, info) && now < cooldown.until
        })
    }
}
