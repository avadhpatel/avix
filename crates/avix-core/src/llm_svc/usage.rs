use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TextUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub requests: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ImageUsage {
    pub requests: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SpeechUsage {
    pub characters_generated: u64,
    pub requests: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TranscriptionUsage {
    pub audio_seconds: f64,
    pub requests: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingUsage {
    pub input_tokens: u64,
    pub requests: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProviderUsage {
    pub text: TextUsage,
    pub image: ImageUsage,
    pub speech: SpeechUsage,
    pub transcription: TranscriptionUsage,
    pub embedding: EmbeddingUsage,
}

#[derive(Debug, Clone, Default)]
pub struct UsageTracker {
    inner: Arc<RwLock<HashMap<String, ProviderUsage>>>,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn record_text(&self, provider: &str, input: u64, output: u64) {
        let mut map = self.inner.write().await;
        let e = map.entry(provider.to_string()).or_default();
        e.text.input_tokens += input;
        e.text.output_tokens += output;
        e.text.requests += 1;
    }

    pub async fn record_text_error(&self, provider: &str) {
        let mut map = self.inner.write().await;
        map.entry(provider.to_string()).or_default().text.errors += 1;
    }

    pub async fn record_image(&self, provider: &str) {
        let mut map = self.inner.write().await;
        map.entry(provider.to_string()).or_default().image.requests += 1;
    }

    pub async fn record_image_error(&self, provider: &str) {
        let mut map = self.inner.write().await;
        map.entry(provider.to_string()).or_default().image.errors += 1;
    }

    pub async fn record_speech(&self, provider: &str, chars: u64) {
        let mut map = self.inner.write().await;
        let e = map.entry(provider.to_string()).or_default();
        e.speech.characters_generated += chars;
        e.speech.requests += 1;
    }

    pub async fn record_speech_error(&self, provider: &str) {
        let mut map = self.inner.write().await;
        map.entry(provider.to_string()).or_default().speech.errors += 1;
    }

    pub async fn record_transcription(&self, provider: &str, audio_sec: f64) {
        let mut map = self.inner.write().await;
        let e = map.entry(provider.to_string()).or_default();
        e.transcription.audio_seconds += audio_sec;
        e.transcription.requests += 1;
    }

    pub async fn record_transcription_error(&self, provider: &str) {
        let mut map = self.inner.write().await;
        map.entry(provider.to_string())
            .or_default()
            .transcription
            .errors += 1;
    }

    pub async fn record_embedding(&self, provider: &str, input_tokens: u64) {
        let mut map = self.inner.write().await;
        let e = map.entry(provider.to_string()).or_default();
        e.embedding.input_tokens += input_tokens;
        e.embedding.requests += 1;
    }

    pub async fn record_embedding_error(&self, provider: &str) {
        let mut map = self.inner.write().await;
        map.entry(provider.to_string())
            .or_default()
            .embedding
            .errors += 1;
    }

    pub async fn snapshot(&self) -> HashMap<String, ProviderUsage> {
        self.inner.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_text_accumulates() {
        let tracker = UsageTracker::new();
        tracker.record_text("openai", 100, 50).await;
        tracker.record_text("openai", 200, 75).await;
        let snap = tracker.snapshot().await;
        let usage = &snap["openai"].text;
        assert_eq!(usage.input_tokens, 300);
        assert_eq!(usage.output_tokens, 125);
        assert_eq!(usage.requests, 2);
        assert_eq!(usage.errors, 0);
    }

    #[tokio::test]
    async fn test_record_text_error() {
        let tracker = UsageTracker::new();
        tracker.record_text_error("anthropic").await;
        tracker.record_text_error("anthropic").await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap["anthropic"].text.errors, 2);
    }

    #[tokio::test]
    async fn test_record_image() {
        let tracker = UsageTracker::new();
        tracker.record_image("openai").await;
        tracker.record_image("openai").await;
        tracker.record_image("openai").await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap["openai"].image.requests, 3);
    }

    #[tokio::test]
    async fn test_record_image_error() {
        let tracker = UsageTracker::new();
        tracker.record_image_error("stability-ai").await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap["stability-ai"].image.errors, 1);
    }

    #[tokio::test]
    async fn test_record_speech_accumulates_chars() {
        let tracker = UsageTracker::new();
        tracker.record_speech("elevenlabs", 100).await;
        tracker.record_speech("elevenlabs", 250).await;
        let snap = tracker.snapshot().await;
        let usage = &snap["elevenlabs"].speech;
        assert_eq!(usage.characters_generated, 350);
        assert_eq!(usage.requests, 2);
    }

    #[tokio::test]
    async fn test_record_transcription_accumulates_seconds() {
        let tracker = UsageTracker::new();
        tracker.record_transcription("openai", 30.5).await;
        tracker.record_transcription("openai", 60.0).await;
        let snap = tracker.snapshot().await;
        let usage = &snap["openai"].transcription;
        assert!((usage.audio_seconds - 90.5).abs() < 0.001);
        assert_eq!(usage.requests, 2);
    }

    #[tokio::test]
    async fn test_record_embedding_accumulates_tokens() {
        let tracker = UsageTracker::new();
        tracker.record_embedding("openai", 512).await;
        tracker.record_embedding("openai", 256).await;
        let snap = tracker.snapshot().await;
        let usage = &snap["openai"].embedding;
        assert_eq!(usage.input_tokens, 768);
        assert_eq!(usage.requests, 2);
    }

    #[tokio::test]
    async fn test_snapshot_returns_all_providers() {
        let tracker = UsageTracker::new();
        tracker.record_text("anthropic", 10, 5).await;
        tracker.record_image("openai").await;
        tracker.record_speech("elevenlabs", 50).await;
        let snap = tracker.snapshot().await;
        assert!(snap.contains_key("anthropic"));
        assert!(snap.contains_key("openai"));
        assert!(snap.contains_key("elevenlabs"));
    }

    #[tokio::test]
    async fn test_snapshot_empty_on_new_tracker() {
        let tracker = UsageTracker::new();
        let snap = tracker.snapshot().await;
        assert!(snap.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_providers_independent() {
        let tracker = UsageTracker::new();
        tracker.record_text("openai", 100, 50).await;
        tracker.record_text("anthropic", 200, 100).await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap["openai"].text.input_tokens, 100);
        assert_eq!(snap["anthropic"].text.input_tokens, 200);
    }

    #[tokio::test]
    async fn test_usage_serializable() {
        let tracker = UsageTracker::new();
        tracker.record_text("openai", 10, 5).await;
        let snap = tracker.snapshot().await;
        // Should be serializable to JSON
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("openai"));
        assert!(json.contains("input_tokens"));
    }

    #[tokio::test]
    async fn test_record_speech_error() {
        let tracker = UsageTracker::new();
        tracker.record_speech_error("elevenlabs").await;
        tracker.record_speech_error("elevenlabs").await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap["elevenlabs"].speech.errors, 2);
    }

    #[tokio::test]
    async fn test_record_transcription_error() {
        let tracker = UsageTracker::new();
        tracker.record_transcription_error("openai").await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap["openai"].transcription.errors, 1);
    }

    #[tokio::test]
    async fn test_record_embedding_error() {
        let tracker = UsageTracker::new();
        tracker.record_embedding_error("openai").await;
        tracker.record_embedding_error("openai").await;
        tracker.record_embedding_error("openai").await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap["openai"].embedding.errors, 3);
    }

    #[tokio::test]
    async fn test_provider_usage_default_is_zero() {
        let usage = ProviderUsage::default();
        assert_eq!(usage.text.input_tokens, 0);
        assert_eq!(usage.text.output_tokens, 0);
        assert_eq!(usage.text.requests, 0);
        assert_eq!(usage.text.errors, 0);
        assert_eq!(usage.image.requests, 0);
        assert_eq!(usage.speech.characters_generated, 0);
        assert_eq!(usage.transcription.audio_seconds, 0.0);
        assert_eq!(usage.embedding.input_tokens, 0);
    }
}
