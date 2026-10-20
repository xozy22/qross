//! Große Inhalte der Zwischenablage beim Wechsel zurückstellen. Bei `Enter` geht eine geänderte
//! Zwischenablage an den Client, genau dann, wenn man drüben die Maus bewegt. Text geht sofort mit;
//! Bilder, kopierte Dateien und alles über 64 KB erst, wenn die Maus nach dem Wechsel 200 ms ruht,
//! höchstens aber 1 s später. Bis dahin gehört die Leitung den ersten Bewegungen drüben.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use qross_proto::ClipboardData;

use crate::clipboard::Content;

/// Größer als das (Text, HTML und Bild zusammen) wartet ein Inhalt, bis die Maus ruht
pub const BULKY: usize = 64 * 1024;
/// So lange muss die Maus nach dem Wechsel ruhen
pub const REST: Duration = Duration::from_millis(200);
/// Länger wartet ein großer Inhalt nie
pub const MAX_WAIT: Duration = Duration::from_secs(1);

/// Wann sich die Maus zuletzt bewegt hat (Bewegungen aus der Erfassung des Servers), für alle
/// Verbindungen gemeinsam
#[derive(Clone)]
pub struct MouseActivity {
    base: Instant,
    /// Nanosekunden seit `base`; 0: noch nie
    last: Arc<AtomicU64>,
}

impl Default for MouseActivity {
    fn default() -> Self {
        Self { base: Instant::now(), last: Arc::new(AtomicU64::new(0)) }
    }
}

impl MouseActivity {
    pub fn moved(&self, at: Instant) {
        let nanos = at.saturating_duration_since(self.base).as_nanos().max(1) as u64;
        self.last.fetch_max(nanos, Ordering::Relaxed);
    }

    fn last(&self) -> Option<Instant> {
        match self.last.load(Ordering::Relaxed) {
            0 => None,
            nanos => Some(self.base + Duration::from_nanos(nanos)),
        }
    }
}

/// Muss `content` warten, bis die Maus ruht? Kopierte Dateien gehen mit `file_limit` 0 gar nicht
/// mit; dann zählt nur, was außer ihnen in der Zwischenablage liegt.
pub fn is_bulky(content: &Content, file_limit: u64) -> bool {
    match content {
        Content::Data(data) => size(data) > BULKY,
        Content::Files { data, .. } => file_limit > 0 || size(data) > BULKY,
    }
}

fn size(data: &ClipboardData) -> usize {
    data.text.as_ref().map_or(0, String::len)
        + data.html.as_ref().map_or(0, String::len)
        + data.png.as_ref().map_or(0, Vec::len)
}

/// Wann ein großer Inhalt nach dem Wechsel um `since` hinaus darf, wenn sich die Maus zuletzt um
/// `moved` bewegt hat: sobald sie `REST` lang ruht, spätestens `MAX_WAIT` nach dem Wechsel
fn due(since: Instant, moved: Option<Instant>) -> Instant {
    let quiet = moved.map_or(since, |moved| moved.max(since));
    (quiet + REST).min(since + MAX_WAIT)
}

/// Wartet, bis die Maus nach dem Wechsel um `since` ruht, höchstens `MAX_WAIT` lang.
pub async fn settled(mouse: &MouseActivity, since: Instant) {
    loop {
        let at = due(since, mouse.last());
        if Instant::now() >= at {
            return;
        }
        tokio::time::sleep_until(at.into()).await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn text_goes_at_once_images_and_files_wait() {
        assert!(!is_bulky(&Content::Data(ClipboardData::from_text("Grüße")), 1000));
        let html = ClipboardData { html: Some("<p>Absatz</p>".repeat(6000)), ..ClipboardData::default() };
        assert!(is_bulky(&Content::Data(html), 1000), "eine ganze Webseite");
        let image = ClipboardData { png: Some(vec![0; 300_000]), ..ClipboardData::default() };
        assert!(is_bulky(&Content::Data(image), 1000));
        let files = Content::Files { paths: vec![PathBuf::from("Bericht.pdf")], data: ClipboardData::from_text("Bericht.pdf") };
        assert!(is_bulky(&files, 1000));
        assert!(!is_bulky(&files, 0), "ohne die Dateien bleiben nur ihre Namen");
    }

    #[test]
    fn large_content_waits_for_the_mouse_to_rest_but_not_long() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(due(t, None), t + REST, "die Maus ruht schon");
        assert_eq!(due(t, Some(t - ms(5))), t + REST, "die Bewegung über die Kante zählt nicht");
        assert_eq!(due(t, Some(t + ms(300))), t + ms(500), "200 ms nach der letzten Bewegung");
        assert_eq!(due(t, Some(t + ms(950))), t + MAX_WAIT, "höchstens 1 s");
    }

    #[tokio::test]
    async fn waiting_ends_once_the_mouse_rests() {
        let mouse = MouseActivity::default();
        let since = Instant::now();
        mouse.moved(since + Duration::from_millis(100));
        settled(&mouse, since).await;
        let waited = since.elapsed();
        assert!(waited >= Duration::from_millis(295) && waited < MAX_WAIT, "{waited:?}");
    }
}
