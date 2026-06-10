use crate::LogEntry;
use tracing::field::Visit;
use tracing::Subscriber;
use tracing_subscriber::Layer;

struct MessageVisitor {
    message: String,
    extra_field: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            if !self.extra_field.is_empty() {
                self.extra_field.push_str(", ");
            }
            self.extra_field
                .push_str(&format!("{}={}", field.name(), value));
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn core::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else {
            if !self.extra_field.is_empty() {
                self.extra_field.push_str(", ");
            }
            self.extra_field
                .push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

pub struct SlintLayer {
    pub sender: tokio::sync::mpsc::Sender<LogEntry>,
}

impl<S: Subscriber> Layer<S> for SlintLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let clean_level = event.metadata().level().to_string().replace('"', "");

        let mut visitor = MessageVisitor {
            message: String::new(),
            extra_field: String::new(),
        };
        event.record(&mut visitor);
        let raw_message = if visitor.extra_field.is_empty() {
            visitor.message
        } else {
            format!("{} {}", visitor.message, visitor.extra_field)
        };

        let mut final_message = raw_message.clone();

        if let Some(msg_start) = raw_message.find(r#""message": String("#) {
            let start_idx = msg_start + r#""message": String("#.len();
            let remainder = &raw_message[start_idx..];

            if let Some(end_idx) = remainder.find(r#"")"#) {
                let extracted_text = &remainder[..end_idx];
                final_message = format!("Youtube Fehler: {}", extracted_text);
            }
        }

        let _ = self.sender.try_send(LogEntry {
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string().into(),
            level: clean_level.into(),
            message: final_message.into(),
        });
    }
}
