use tokio::sync::broadcast;

/// Events that flow through the daemon event bus.
#[derive(Debug, Clone)]
pub enum Event {
    /// A screenshot was captured and stored.
    ScreenshotCaptured {
        timestamp: i64,
        monitor: String,
        screenshot_filename: String,
    },
    /// A custom event emitted by a plugin or external source.
    Custom {
        source: String,
        name: String,
        payload: Vec<u8>,
    },
    /// Timer tick (for plugins that want periodic callbacks).
    TimerTick,
}

/// Actions a plugin can request from the host.
#[derive(Debug, Clone)]
pub enum Action {
    /// Request an immediate screenshot capture.
    RequestScreenshot,
    /// Submit text to be embedded and stored alongside the current context.
    SubmitText {
        app: String,
        title: String,
        text: String,
    },
    /// Log a message at info level.
    Log(String),
}

/// Central event bus backed by a tokio broadcast channel.
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish an event to all subscribers.
    pub fn publish(&self, event: Event) {
        // It's OK if nobody is listening.
        let _ = self.tx.send(event);
    }

    /// Create a new subscriber.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}
