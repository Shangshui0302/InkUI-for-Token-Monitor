//! Business-protocol multiplexing over an authenticated [`OnlineSession`].
//!
//! A single actor owns the session and is the only code that reads or writes
//! it. Client requests, inbound server requests, and events remain independently
//! bounded.

use crate::discovery::{HEARTBEAT_INTERVAL, OFFLINE_AFTER};
use crate::transport::{MembershipAuthorizer, OnlineSession, TransportError, TransportIo};
use nekodex_protocol::{
    BusinessCapabilities, BusinessFrame, Event, EventEnvelope, Operation, ProtocolError,
    ProtocolVersion, RequestEnvelope, ResponseEnvelope,
};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior};

#[derive(Debug, thiserror::Error)]
pub enum BusinessError {
    #[error("business request queue is full")]
    TooManyPending,
    #[error("business command queue is full")]
    CommandQueueFull,
    #[error("business event queue is full")]
    EventQueueFull,
    #[error("business request timed out")]
    Timeout,
    #[error("business request id is already pending")]
    DuplicateRequest,
    #[error("business session closed")]
    Closed,
    #[error("business protocol encoding failed")]
    Encoding(#[from] serde_json::Error),
    #[error("business envelope does not match the negotiated protocol version")]
    IncompatibleProtocol,
    #[error("operation is not supported by the negotiated capabilities: {0:?}")]
    UnsupportedCapability(Operation),
    #[error("transport: {0}")]
    Transport(#[from] TransportError),
}

#[derive(Clone, Copy, Debug)]
pub struct BusinessLimits {
    pub max_pending: usize,
    pub command_queue: usize,
    pub request_queue: usize,
    pub event_queue: usize,
}

impl Default for BusinessLimits {
    fn default() -> Self {
        Self {
            max_pending: 32,
            command_queue: 32,
            request_queue: 32,
            event_queue: 32,
        }
    }
}

pub struct BusinessConnection;

impl BusinessConnection {
    pub fn spawn<S: TransportIo + 'static>(
        session: OnlineSession<S>,
        limits: BusinessLimits,
        authorizer: Arc<dyn MembershipAuthorizer>,
    ) -> (BusinessClient, BusinessServer, BusinessEvents, MembershipEvents) {
        assert!(limits.max_pending > 0, "max_pending must be non-zero");
        assert!(limits.command_queue > 0, "command_queue must be non-zero");
        assert!(limits.request_queue > 0, "request_queue must be non-zero");
        assert!(limits.event_queue > 0, "event_queue must be non-zero");

        let (commands, command_rx) = mpsc::channel(limits.command_queue);
        let (requests, request_rx) = mpsc::channel(limits.request_queue);
        let (events, event_rx) = mpsc::channel(limits.event_queue);
        let (priority_events, priority_event_rx) = mpsc::channel(limits.command_queue);
        let (recovery, recovery_rx) = mpsc::channel(1);
        let (membership_updates, membership_update_rx) = mpsc::channel(limits.command_queue);
        let (event_commands, event_command_rx) = mpsc::channel(limits.event_queue);
        let (priority_event_commands, priority_event_command_rx) =
            mpsc::channel(limits.command_queue);
        let (recovery_event_commands, recovery_event_command_rx) = mpsc::channel(1);
        let shutdown = Arc::new(Shutdown::default());
        let negotiated_capabilities = session.negotiated_capabilities().clone();
        tokio::spawn(run_actor(
            session,
            command_rx,
            requests,
            events,
            priority_events,
            recovery,
            event_command_rx,
            priority_event_command_rx,
            recovery_event_command_rx,
            Arc::clone(&shutdown),
            limits.max_pending,
            negotiated_capabilities,
            authorizer,
            membership_updates,
        ));

        (
            BusinessClient {
                commands: commands.clone(),
                event_commands,
                priority_event_commands,
                recovery_event_commands,
                shutdown: Arc::clone(&shutdown),
                next_token: Arc::new(AtomicU64::new(1)),
            },
            BusinessServer {
                commands,
                requests: request_rx,
                shutdown: Arc::clone(&shutdown),
            },
            BusinessEvents {
                events: event_rx,
                priority_events: priority_event_rx,
                recovery: recovery_rx,
                events_closed: false,
                priority_events_closed: false,
                recovery_closed: false,
                shutdown,
            },
            MembershipEvents {
                updates: membership_update_rx,
            },
        )
    }
}

#[derive(Clone)]
pub struct BusinessClient {
    commands: mpsc::Sender<Command>,
    event_commands: mpsc::Sender<EventCommand>,
    priority_event_commands: mpsc::Sender<EventCommand>,
    recovery_event_commands: mpsc::Sender<EventEnvelope>,
    shutdown: Arc<Shutdown>,
    next_token: Arc<AtomicU64>,
}

impl BusinessClient {
    pub fn close(&self) {
        self.shutdown.close();
    }

    pub fn queue_membership_update(&self, bundle: Vec<u8>) -> Result<(), BusinessError> {
        self.send_command(Command::MembershipUpdate { bundle })
    }

    pub async fn request(
        &self,
        request: RequestEnvelope,
        timeout: Duration,
    ) -> Result<ResponseEnvelope, BusinessError> {
        let request_id = request.request_id.clone();
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let (reply, response) = oneshot::channel();
        self.send_command(Command::Request {
            request,
            token,
            reply,
        })?;

        match tokio::time::timeout(timeout, response).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(BusinessError::Closed),
            Err(_) => {
                let (reply, cancelled) = oneshot::channel();
                if self
                    .send_command(Command::Cancel {
                        request_id,
                        token,
                        reply,
                    })
                    .is_ok()
                {
                    let _ = cancelled.await;
                }
                Err(BusinessError::Timeout)
            }
        }
    }

    pub async fn send_event(&self, event: EventEnvelope) -> Result<(), BusinessError> {
        self.enqueue_event(event)?
            .await
            .map_err(|_| BusinessError::Closed)?
    }

    /// Queue an event without waiting for socket I/O. The bounded actor queue
    /// still applies backpressure and coalesces a recovery marker when full,
    /// but one slow peer cannot stall broadcasting to the rest of the group.
    pub fn queue_event(&self, event: EventEnvelope) -> Result<(), BusinessError> {
        drop(self.enqueue_event(event)?);
        Ok(())
    }

    fn enqueue_event(
        &self,
        event: EventEnvelope,
    ) -> Result<oneshot::Receiver<Result<(), BusinessError>>, BusinessError> {
        let (reply, result) = oneshot::channel();
        let command = EventCommand::Event { event, reply };
        if is_priority_event(&command) {
            if self.shutdown.closed.load(Ordering::Acquire) {
                return Err(BusinessError::Closed);
            }
            match self.priority_event_commands.try_send(command) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(EventCommand::Event { event, reply })) => {
                    drop(reply);
                    self.queue_recovery(&event)?;
                    return Err(BusinessError::EventQueueFull);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    return Err(BusinessError::Closed);
                }
            }
        } else {
            if self.shutdown.closed.load(Ordering::Acquire) {
                return Err(BusinessError::Closed);
            }
            match self.event_commands.try_send(command) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(EventCommand::Event { event, reply })) => {
                    drop(reply);
                    self.queue_recovery(&event)?;
                    return Err(BusinessError::EventQueueFull);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    return Err(BusinessError::Closed);
                }
            }
        }
        Ok(result)
    }

    fn queue_recovery(&self, event: &EventEnvelope) -> Result<(), BusinessError> {
        match self
            .recovery_event_commands
            .try_send(snapshot_required_for(event))
        {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(BusinessError::Closed),
        }
    }

    fn send_command(&self, command: Command) -> Result<(), BusinessError> {
        if self.shutdown.closed.load(Ordering::Acquire) {
            return Err(BusinessError::Closed);
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => BusinessError::CommandQueueFull,
                mpsc::error::TrySendError::Closed(_) => BusinessError::Closed,
            })
    }
}

pub struct BusinessEvents {
    events: mpsc::Receiver<EventEnvelope>,
    priority_events: mpsc::Receiver<EventEnvelope>,
    recovery: mpsc::Receiver<EventEnvelope>,
    events_closed: bool,
    priority_events_closed: bool,
    recovery_closed: bool,
    shutdown: Arc<Shutdown>,
}

pub struct MembershipEvents {
    updates: mpsc::Receiver<()>,
}

impl MembershipEvents {
    pub async fn receive(&mut self) -> Result<(), BusinessError> {
        self.updates.recv().await.ok_or(BusinessError::Closed)
    }
}

impl BusinessEvents {
    pub async fn receive_event(&mut self) -> Result<EventEnvelope, BusinessError> {
        loop {
            if self.events_closed && self.priority_events_closed && self.recovery_closed {
                return Err(BusinessError::Closed);
            }
            tokio::select! {
                event = self.recovery.recv(), if !self.recovery_closed => {
                    match event {
                        Some(event) => return Ok(event),
                        None => self.recovery_closed = true,
                    }
                }
                event = self.priority_events.recv(), if !self.priority_events_closed => {
                    match event {
                        Some(event) => return Ok(event),
                        None => self.priority_events_closed = true,
                    }
                }
                event = self.events.recv(), if !self.events_closed => {
                    match event {
                        Some(event) => return Ok(event),
                        None => self.events_closed = true,
                    }
                }
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.shutdown.closed.load(Ordering::Acquire)
    }
}

pub struct BusinessServer {
    commands: mpsc::Sender<Command>,
    requests: mpsc::Receiver<RequestEnvelope>,
    shutdown: Arc<Shutdown>,
}

impl BusinessServer {
    pub async fn receive_request(&mut self) -> Result<RequestEnvelope, BusinessError> {
        self.requests.recv().await.ok_or(BusinessError::Closed)
    }

    pub async fn send_response(&self, response: ResponseEnvelope) -> Result<(), BusinessError> {
        let (reply, result) = oneshot::channel();
        self.send_command(Command::Response { response, reply })?;
        result.await.map_err(|_| BusinessError::Closed)?
    }

    fn send_command(&self, command: Command) -> Result<(), BusinessError> {
        if self.shutdown.closed.load(Ordering::Acquire) {
            return Err(BusinessError::Closed);
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => BusinessError::CommandQueueFull,
                mpsc::error::TrySendError::Closed(_) => BusinessError::Closed,
            })
    }
}

#[derive(Default)]
struct Shutdown {
    closed: AtomicBool,
    notify: Notify,
}

impl Shutdown {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

enum Command {
    MembershipUpdate {
        bundle: Vec<u8>,
    },
    Request {
        request: RequestEnvelope,
        token: u64,
        reply: oneshot::Sender<Result<ResponseEnvelope, BusinessError>>,
    },
    Cancel {
        request_id: String,
        token: u64,
        reply: oneshot::Sender<()>,
    },
    Response {
        response: ResponseEnvelope,
        reply: oneshot::Sender<Result<(), BusinessError>>,
    },
}

enum EventCommand {
    Event {
        event: EventEnvelope,
        reply: oneshot::Sender<Result<(), BusinessError>>,
    },
}

fn is_priority_event(command: &EventCommand) -> bool {
    match command {
        EventCommand::Event { event, .. } => is_priority_event_kind(&event.event),
    }
}

fn is_priority_event_kind(event: &Event) -> bool {
    !matches!(event, Event::ActivitySnapshot(_))
}

struct Pending {
    token: u64,
    reply: oneshot::Sender<Result<ResponseEnvelope, BusinessError>>,
}

async fn run_actor<S: TransportIo>(
    mut session: OnlineSession<S>,
    mut commands: mpsc::Receiver<Command>,
    requests: mpsc::Sender<RequestEnvelope>,
    events: mpsc::Sender<EventEnvelope>,
    priority_events: mpsc::Sender<EventEnvelope>,
    recovery: mpsc::Sender<EventEnvelope>,
    mut event_commands: mpsc::Receiver<EventCommand>,
    mut priority_event_commands: mpsc::Receiver<EventCommand>,
    mut recovery_event_commands: mpsc::Receiver<EventEnvelope>,
    shutdown: Arc<Shutdown>,
    max_pending: usize,
    negotiated_capabilities: BusinessCapabilities,
    authorizer: Arc<dyn MembershipAuthorizer>,
    membership_updates: mpsc::Sender<()>,
) {
    let negotiated_protocol = negotiated_capabilities.protocol;
    let mut pending = HashMap::<String, Pending>::new();
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_seen = Instant::now();
    let mut heartbeat_sequence = 0u64;
    let mut event_commands_open = true;
    let mut priority_event_commands_open = true;
    let mut recovery_event_commands_open = true;
    let mut ordinary_gap = false;

    loop {
        if shutdown.closed.load(Ordering::Acquire) {
            break;
        }
        tokio::select! {
            _ = shutdown.notify.notified() => {
                if shutdown.closed.load(Ordering::Acquire) {
                    break;
                }
            }
            event = recovery_event_commands.recv(), if recovery_event_commands_open => {
                match event {
                    Some(event) => {
                        if event.version != negotiated_protocol
                            || send_frame(&mut session, BusinessFrame::Event(event)).await.is_err()
                        {
                            break;
                        }
                    }
                    None => recovery_event_commands_open = false,
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                if handle_command(&mut session, &mut pending, command, max_pending, negotiated_protocol).await.is_err() {
                    break;
                }
            }
            command = priority_event_commands.recv(), if priority_event_commands_open => {
                match command {
                    Some(command) => {
                        if handle_event_command(&mut session, command, negotiated_protocol).await.is_err() {
                            break;
                        }
                    }
                    None => priority_event_commands_open = false,
                }
            }
            command = event_commands.recv(), if event_commands_open => {
                match command {
                    Some(command) => {
                        if handle_event_command(&mut session, command, negotiated_protocol).await.is_err() {
                            break;
                        }
                    }
                    None => event_commands_open = false,
                }
            }
            frame = session.receive() => {
                let Ok(bytes) = frame else { break };
                let Ok(frame) = serde_json::from_slice::<BusinessFrame>(&bytes) else { break };
                last_seen = Instant::now();
                let admitted = match frame {
                    BusinessFrame::Response(response) => {
                        if response.version != negotiated_protocol {
                            if let Some(waiter) = pending.remove(&response.request_id) {
                                let _ = waiter.reply.send(Err(BusinessError::IncompatibleProtocol));
                            }
                            false
                        } else {
                            if let Some(waiter) = pending.remove(&response.request_id) {
                                let _ = waiter.reply.send(Ok(response));
                            }
                            true
                        }
                    }
                    BusinessFrame::Request(request) => {
                        if request.version != negotiated_protocol {
                            send_frame(
                                &mut session,
                                BusinessFrame::Response(ResponseEnvelope {
                                    version: negotiated_protocol,
                                    request_id: request.request_id,
                                    result: None,
                                    error: Some(ProtocolError::UnsupportedProtocolVersion {
                                        supported: negotiated_protocol,
                                        actual: request.version,
                                    }),
                                }),
                            )
                            .await
                            .is_ok()
                        } else if !negotiated_capabilities.supports(&request.operation) {
                            let operation = request.operation.clone();
                            send_frame(
                                &mut session,
                                BusinessFrame::Response(ResponseEnvelope {
                                    version: negotiated_protocol,
                                    request_id: request.request_id,
                                    result: None,
                                    error: Some(ProtocolError::MissingCapability {
                                        capability: operation.capability_id().to_owned(),
                                        operation,
                                    }),
                                }),
                            )
                            .await
                            .is_ok()
                        } else {
                            match requests.try_send(request) {
                                Ok(()) => true,
                                Err(mpsc::error::TrySendError::Full(request))
                                | Err(mpsc::error::TrySendError::Closed(request)) => {
                                    send_frame(
                                        &mut session,
                                        BusinessFrame::Response(ResponseEnvelope {
                                            version: request.version,
                                            request_id: request.request_id,
                                            result: None,
                                            error: Some(ProtocolError::SourceUnavailable),
                                        }),
                                    )
                                    .await
                                    .is_ok()
                                }
                            }
                        }
                    }
                    BusinessFrame::Event(event) => {
                        if event.version != negotiated_protocol {
                            false
                        } else {
                            if is_priority_event_kind(&event.event) {
                                match priority_events.try_send(event) {
                                    Ok(()) => ordinary_gap = false,
                                    Err(mpsc::error::TrySendError::Full(event)) => {
                                        if !ordinary_gap {
                                            let snapshot_required = snapshot_required_for(&event);
                                            let _ = recovery.try_send(snapshot_required);
                                            ordinary_gap = true;
                                        }
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                                }
                            } else {
                                match events.try_send(event) {
                                    Ok(()) => ordinary_gap = false,
                                    Err(mpsc::error::TrySendError::Full(event)) => {
                                        if !ordinary_gap {
                                            let snapshot_required = snapshot_required_for(&event);
                                            let _ = recovery.try_send(snapshot_required);
                                            ordinary_gap = true;
                                        }
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                                }
                            }
                            true
                        }
                    }
                    BusinessFrame::MembershipUpdate(bundle) => {
                        if authorizer
                            .apply_sync(session.group_id(), &session.peer_device_id, &bundle)
                            .await
                            .is_err()
                        {
                            false
                        } else {
                            membership_updates.try_send(()).is_ok()
                        }
                    }
                    BusinessFrame::Ping { sequence } => {
                        send_frame(&mut session, BusinessFrame::Pong { sequence }).await.is_ok()
                    }
                    BusinessFrame::Pong { .. } => true,
                };
                if !admitted {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                if last_seen.elapsed() >= OFFLINE_AFTER {
                    break;
                }
                heartbeat_sequence = heartbeat_sequence.wrapping_add(1);
                if send_frame(
                    &mut session,
                    BusinessFrame::Ping { sequence: heartbeat_sequence },
                )
                .await
                .is_err()
                {
                    break;
                }
            }
        }
    }

    shutdown.close();
}

async fn handle_command<S: TransportIo>(
    session: &mut OnlineSession<S>,
    pending: &mut HashMap<String, Pending>,
    command: Command,
    max_pending: usize,
    negotiated_protocol: ProtocolVersion,
) -> Result<(), ()> {
    match command {
        Command::MembershipUpdate { bundle } => {
            if send_frame(session, BusinessFrame::MembershipUpdate(bundle))
                .await
                .is_err()
            {
                return Err(());
            }
        }
        Command::Request {
            request,
            token,
            reply,
        } => {
            if request.version != negotiated_protocol {
                let _ = reply.send(Err(BusinessError::IncompatibleProtocol));
                return Ok(());
            }
            if !session
                .negotiated_capabilities()
                .supports(&request.operation)
            {
                let _ = reply.send(Err(BusinessError::UnsupportedCapability(request.operation)));
                return Ok(());
            }
            if pending.len() >= max_pending {
                let _ = reply.send(Err(BusinessError::TooManyPending));
                return Ok(());
            }
            if pending.contains_key(&request.request_id) {
                let _ = reply.send(Err(BusinessError::DuplicateRequest));
                return Ok(());
            }
            let request_id = request.request_id.clone();
            if let Err(error) = send_frame(session, BusinessFrame::Request(request)).await {
                let _ = reply.send(Err(error));
                return Err(());
            }
            pending.insert(request_id, Pending { token, reply });
        }
        Command::Cancel {
            request_id,
            token,
            reply,
        } => {
            if pending
                .get(&request_id)
                .is_some_and(|entry| entry.token == token)
            {
                pending.remove(&request_id);
            }
            let _ = reply.send(());
        }
        Command::Response { response, reply } => {
            if response.version != negotiated_protocol {
                let _ = reply.send(Err(BusinessError::IncompatibleProtocol));
                return Ok(());
            }
            let fallback = ResponseEnvelope {
                version: response.version,
                request_id: response.request_id.clone(),
                result: None,
                error: Some(ProtocolError::SourceUnavailable),
            };
            let mut result = send_frame(session, BusinessFrame::Response(response)).await;
            if matches!(
                result,
                Err(BusinessError::Transport(TransportError::Oversized))
            ) {
                result = send_frame(session, BusinessFrame::Response(fallback)).await;
            }
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed {
                return Err(());
            }
        }
    }
    Ok(())
}

async fn handle_event_command<S: TransportIo>(
    session: &mut OnlineSession<S>,
    command: EventCommand,
    negotiated_protocol: ProtocolVersion,
) -> Result<(), ()> {
    match command {
        EventCommand::Event { event, reply } => {
            if event.version != negotiated_protocol {
                let _ = reply.send(Err(BusinessError::IncompatibleProtocol));
                return Ok(());
            }
            let result = send_frame(session, BusinessFrame::Event(event)).await;
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed {
                return Err(());
            }
        }
    }
    Ok(())
}

fn snapshot_required_for(event: &EventEnvelope) -> EventEnvelope {
    EventEnvelope {
        version: event.version,
        source_device_id: event.source_device_id.clone(),
        process_generation: event.process_generation,
        cursor: event.cursor,
        event: Event::SnapshotRequired {
            oldest_available: event.cursor,
        },
    }
}

async fn send_frame<S: TransportIo>(
    session: &mut OnlineSession<S>,
    frame: BusinessFrame,
) -> Result<(), BusinessError> {
    session.enqueue(serde_json::to_vec(&frame)?)?;
    session.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nekodex_protocol::{ActivitySnapshot, Cursor, ProcessGeneration, ProtocolVersion};

    fn activity_event(cursor: u64) -> EventEnvelope {
        EventEnvelope {
            version: ProtocolVersion::V1,
            source_device_id: "source".into(),
            process_generation: ProcessGeneration(1),
            cursor: Cursor(cursor),
            event: Event::ActivitySnapshot(ActivitySnapshot {
                source_device_id: "source".into(),
                activities: Vec::new(),
                observed_at_unix_seconds: 1,
                next: None,
            }),
        }
    }

    #[test]
    fn nonblocking_event_queue_preserves_bounded_recovery_signal() {
        let (commands, _commands_rx) = mpsc::channel(1);
        let (event_commands, mut event_commands_rx) = mpsc::channel(1);
        let (priority_event_commands, _priority_event_commands_rx) = mpsc::channel(1);
        let (recovery_event_commands, mut recovery_event_commands_rx) = mpsc::channel(1);
        let client = BusinessClient {
            commands,
            event_commands,
            priority_event_commands,
            recovery_event_commands,
            shutdown: Arc::new(Shutdown::default()),
            next_token: Arc::new(AtomicU64::new(1)),
        };

        client.queue_event(activity_event(1)).unwrap();
        assert!(matches!(
            client.queue_event(activity_event(2)),
            Err(BusinessError::EventQueueFull)
        ));
        assert!(event_commands_rx.try_recv().is_ok());
        let recovery = recovery_event_commands_rx.try_recv().unwrap();
        assert!(matches!(
            recovery.event,
            Event::SnapshotRequired {
                oldest_available: Cursor(2)
            }
        ));
    }
}
