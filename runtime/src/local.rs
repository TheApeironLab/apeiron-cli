//! A socket binds once. Its immutable permit is retained by every queued task.
//! No process/boot identifier is interpreted as evidence of child termination.
use crate::cloud::{fresh, now_ms, Permit, WINDOW_MS};
use crate::protocol::{Inbound, LocalAnnouncement, LocalScope, Outbound, PROTOCOL_VERSION};
use crate::receiver::Receiver;
use std::io;
use std::sync::Arc;

pub struct Authority {
    pub permit: Arc<Permit>,
    identity: Option<(String, String)>,
    scope: Option<LocalScope>,
    nonce: String,
    issued: u64,
    accepted: bool,
    waiting: bool,
}

impl Authority {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            permit: Arc::new(Permit::new()),
            identity: None,
            scope: None,
            nonce: fresh()?,
            issued: now_ms()?,
            accepted: false,
            waiting: true,
        })
    }

    pub fn check(&self) -> io::Result<()> {
        if !self.accepted {
            return Err(io::Error::other("local receiver is not authorized"));
        }
        self.permit.check()
    }

    pub fn tick(&mut self) -> io::Result<Option<Outbound>> {
        let now = now_ms()?;
        if self.accepted {
            self.permit.check()?;
        } else if now >= self.issued + WINDOW_MS {
            return Err(io::Error::other("local handshake expired"));
        }
        if self.scope.is_some() && !self.waiting && now >= self.issued + WINDOW_MS / 2 {
            self.nonce = fresh()?;
            self.issued = now;
            self.waiting = true;
            return Ok(Some(Outbound::PermitChallenge {
                nonce: self.nonce.clone(),
                window_ms: WINDOW_MS,
            }));
        }
        Ok(None)
    }

    pub fn control(
        &mut self,
        message: &Inbound,
        receiver: &Receiver,
        info: &crate::protocol::MachineInfo,
    ) -> io::Result<Option<Vec<Outbound>>> {
        match message {
            Inbound::Welcome {
                protocol,
                user_id,
                tenant_id,
            } => {
                if *protocol != PROTOCOL_VERSION
                    || self.identity.is_some()
                    || user_id.is_empty()
                    || tenant_id.is_empty()
                {
                    return Err(io::Error::other("local protocol or identity mismatch"));
                }
                self.identity = Some((tenant_id.clone(), user_id.clone()));
                Ok(Some(vec![Outbound::Ready {
                    protocol: PROTOCOL_VERSION,
                    info: info.clone(),
                    authority: Some(LocalAnnouncement {
                        receiver: receiver.local_hello()?,
                        nonce: self.nonce.clone(),
                        window_ms: WINDOW_MS,
                    }),
                }]))
            }
            Inbound::Bind { scope, nonce } => {
                if self.scope.is_some()
                    || self.identity.as_ref() != Some(&(scope.tenant_id.clone(), scope.user_id.clone()))
                    || nonce != &self.nonce
                    || now_ms()? >= self.issued + WINDOW_MS
                {
                    return Err(io::Error::other("local binding is stale or mismatched"));
                }
                let previous = receiver.bind_local(scope)?;
                self.scope = Some(scope.clone());
                self.nonce = fresh()?;
                self.issued = now_ms()?;
                Ok(Some(vec![
                    Outbound::Bound {
                        scope: scope.clone(),
                        previous,
                    },
                    Outbound::PermitChallenge {
                        nonce: self.nonce.clone(),
                        window_ms: WINDOW_MS,
                    },
                ]))
            }
            Inbound::Permit { nonce } => {
                if self.scope.is_none() || !self.waiting || nonce != &self.nonce {
                    return Err(io::Error::other("local permit nonce mismatch or replay"));
                }
                self.permit.grant(self.issued, now_ms()?)?;
                self.accepted = true;
                self.waiting = false;
                Ok(Some(vec![Outbound::PermitAccepted { nonce: nonce.clone() }]))
            }
            _ => {
                self.check()?;
                Ok(None)
            }
        }
    }
}
