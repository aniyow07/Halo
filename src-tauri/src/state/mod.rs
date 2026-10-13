//! Application state machine.
//!
//! One selection session exists at a time. A session owns the frozen frame
//! it was selected from, the processed image ("visual context"), and the
//! conversation history. At most one model request is in flight. Every
//! session and request carries a unique id, and every mutation checks those
//! ids, so a late event from an old session or a cancelled request can never
//! modify newer state.

use std::sync::{Arc, Mutex, MutexGuard};

use image::RgbaImage;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::capture::CaptureInfo;
use crate::errors::{ErrorKind, HaloError, HaloResult};
use crate::gemini::{GenerateRequest, Turn, VisualContext, build_request};
use crate::image_processing::ProcessingStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Phase {
    Idle,
    SelectingRegion,
    Capturing,
    ProcessingImage,
    WaitingForModel,
    StreamingResponse,
    Completed,
    Error,
    Cancelled,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub struct Session {
    pub id: String,
    pub frame: Arc<RgbaImage>,
    pub capture: CaptureInfo,
    /// Incremented for every new selection within the session, so answers
    /// about a previous selection are never attached to the new one.
    pub context_id: u64,
    pub visual: Option<VisualContext>,
    pub stats: Option<ProcessingStats>,
    pub history: Vec<Turn>,
}

#[derive(Clone, Debug)]
pub struct ActiveRequest {
    pub id: String,
    pub session_id: String,
    pub context_id: u64,
    pub question: String,
    pub cancel: CancellationToken,
}

#[derive(Default)]
struct Inner {
    phase: Option<Phase>,
    activating: bool,
    session: Option<Session>,
    active: Option<ActiveRequest>,
}

#[derive(Default)]
pub struct HaloState {
    inner: Mutex<Inner>,
}

fn stale() -> HaloError {
    HaloError::new(
        ErrorKind::StaleSession,
        "This selection is no longer active. Start a new selection.",
    )
}

impl HaloState {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock must not brick the app; the state
        // is always left consistent between statements.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn phase(&self) -> Phase {
        self.lock().phase.unwrap_or(Phase::Idle)
    }

    pub fn set_phase(&self, phase: Phase) {
        self.lock().phase = Some(phase);
    }

    /// Start a new activation. Rejects overlapping activations and cancels
    /// any in-flight request from the previous session.
    pub fn begin_activation(&self) -> HaloResult<()> {
        let mut g = self.lock();
        if g.activating {
            return Err(HaloError::new(
                ErrorKind::Busy,
                "Halo is already capturing the screen.",
            ));
        }
        if g.session.is_some() && g.active.is_none() && g.phase == Some(Phase::SelectingRegion) {
            return Err(HaloError::new(
                ErrorKind::Busy,
                "A selection is already in progress.",
            ));
        }
        if let Some(a) = g.active.take() {
            a.cancel.cancel();
        }
        g.session = None;
        g.activating = true;
        g.phase = Some(Phase::Capturing);
        Ok(())
    }

    pub fn fail_activation(&self) {
        let mut g = self.lock();
        g.activating = false;
        g.session = None;
        g.phase = Some(Phase::Error);
    }

    pub fn install_session(&self, session: Session) {
        let mut g = self.lock();
        g.activating = false;
        g.session = Some(session);
        g.phase = Some(Phase::SelectingRegion);
    }

    pub fn has_session(&self) -> bool {
        self.lock().session.is_some()
    }

    pub fn session_id(&self) -> Option<String> {
        self.lock().session.as_ref().map(|s| s.id.clone())
    }

    pub fn with_session<T>(
        &self,
        session_id: &str,
        f: impl FnOnce(&Session) -> T,
    ) -> HaloResult<T> {
        let g = self.lock();
        match &g.session {
            Some(s) if s.id == session_id => Ok(f(s)),
            _ => Err(stale()),
        }
    }

    /// Get the frozen frame for processing (cheap Arc clone).
    pub fn frame(&self, session_id: &str) -> HaloResult<Arc<RgbaImage>> {
        self.with_session(session_id, |s| s.frame.clone())
    }

    /// The user (re)started a selection: drop the previous visual context,
    /// history and any in-flight request.
    pub fn begin_selection(&self, session_id: &str) -> HaloResult<u64> {
        let mut g = self.lock();
        let active = g.active.take();
        let s = match g.session.as_mut() {
            Some(s) if s.id == session_id => s,
            _ => {
                g.active = active;
                return Err(stale());
            }
        };
        if let Some(a) = active {
            a.cancel.cancel();
        }
        s.context_id += 1;
        s.visual = None;
        s.stats = None;
        s.history.clear();
        let ctx = s.context_id;
        g.phase = Some(Phase::ProcessingImage);
        Ok(ctx)
    }

    pub fn set_visual(
        &self,
        session_id: &str,
        context_id: u64,
        visual: VisualContext,
        stats: ProcessingStats,
    ) -> HaloResult<()> {
        let mut g = self.lock();
        match g.session.as_mut() {
            Some(s) if s.id == session_id && s.context_id == context_id => {
                s.visual = Some(visual);
                s.stats = Some(stats);
                g.phase = Some(Phase::Idle);
                Ok(())
            }
            _ => Err(stale()),
        }
    }

    /// Register a new request and build its payload from the current visual
    /// context and history.
    pub fn begin_request(
        &self,
        session_id: &str,
        question: &str,
    ) -> HaloResult<(ActiveRequest, GenerateRequest)> {
        let question = question.trim();
        if question.is_empty() {
            return Err(HaloError::invalid_input("Type a question first."));
        }
        if question.chars().count() > 8000 {
            return Err(HaloError::invalid_input(
                "Questions are limited to 8000 characters.",
            ));
        }
        let mut g = self.lock();
        if g.active.is_some() {
            return Err(HaloError::new(
                ErrorKind::Busy,
                "Halo is still answering. Cancel it or wait for it to finish.",
            ));
        }
        let s = match &g.session {
            Some(s) if s.id == session_id => s,
            _ => return Err(stale()),
        };
        let visual = s.visual.as_ref().ok_or_else(|| {
            HaloError::new(
                ErrorKind::InvalidSelection,
                "Select a region before asking a question.",
            )
        })?;
        let payload = build_request(visual, &s.history, question);
        let req = ActiveRequest {
            id: new_id(),
            session_id: s.id.clone(),
            context_id: s.context_id,
            question: question.to_string(),
            cancel: CancellationToken::new(),
        };
        g.active = Some(req.clone());
        g.phase = Some(Phase::WaitingForModel);
        Ok((req, payload))
    }

    /// True if `request_id` is still the active request (used to drop
    /// events from superseded requests).
    pub fn is_active(&self, request_id: &str) -> bool {
        self.lock()
            .active
            .as_ref()
            .is_some_and(|a| a.id == request_id)
    }

    /// Finish a request. The answer is committed to history only if the
    /// request is still the active one and its session/selection are still
    /// current. Returns whether the request was still active.
    pub fn finish_request(&self, request_id: &str, answer: Option<String>, phase: Phase) -> bool {
        let mut g = self.lock();
        let Some(active) = g.active.take_if(|a| a.id == request_id) else {
            return false;
        };
        if let (Some(answer), Some(s)) = (answer, g.session.as_mut())
            && s.id == active.session_id
            && s.context_id == active.context_id
        {
            s.history.push(Turn {
                question: active.question,
                answer,
            });
        }
        g.phase = Some(phase);
        true
    }

    pub fn cancel_request(&self, request_id: &str) -> bool {
        let g = self.lock();
        match &g.active {
            Some(a) if a.id == request_id => {
                a.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// End the session (dismiss). Cancels any in-flight request.
    pub fn end_session(&self, session_id: Option<&str>) -> bool {
        let mut g = self.lock();
        if let (Some(id), Some(s)) = (session_id, &g.session)
            && s.id != id
        {
            return false;
        }
        if let Some(a) = g.active.take() {
            a.cancel.cancel();
        }
        let had = g.session.take().is_some();
        g.activating = false;
        g.phase = Some(Phase::Idle);
        had
    }

    pub fn history_len(&self, session_id: &str) -> HaloResult<usize> {
        self.with_session(session_id, |s| s.history.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CaptureInfo;
    use crate::image_processing::ImageFormat;

    fn session(id: &str) -> Session {
        Session {
            id: id.into(),
            frame: Arc::new(RgbaImage::new(10, 10)),
            capture: CaptureInfo::test_info(10, 10),
            context_id: 0,
            visual: None,
            stats: None,
            history: Vec::new(),
        }
    }

    fn stats() -> ProcessingStats {
        ProcessingStats {
            original_width: 1,
            original_height: 1,
            processed_width: 1,
            processed_height: 1,
            original_bytes: 1,
            processed_bytes: 1,
            reduction_percent: 0.0,
            duration_ms: 0.0,
            format: ImageFormat::Png,
        }
    }

    fn ready(state: &HaloState, id: &str) {
        state.begin_activation().unwrap();
        state.install_session(session(id));
        let ctx = state.begin_selection(id).unwrap();
        state
            .set_visual(id, ctx, VisualContext::new("image/png", b"x"), stats())
            .unwrap();
    }

    #[test]
    fn rejects_overlapping_activation() {
        let s = HaloState::default();
        s.begin_activation().unwrap();
        assert_eq!(s.begin_activation().unwrap_err().kind, ErrorKind::Busy);
        s.install_session(session("a"));
        assert_eq!(s.phase(), Phase::SelectingRegion);
        assert_eq!(s.begin_activation().unwrap_err().kind, ErrorKind::Busy);
        s.end_session(Some("a"));
        assert!(s.begin_activation().is_ok());
    }

    #[test]
    fn failed_activation_allows_recovery() {
        let s = HaloState::default();
        s.begin_activation().unwrap();
        s.fail_activation();
        assert_eq!(s.phase(), Phase::Error);
        assert!(s.begin_activation().is_ok());
    }

    #[test]
    fn requests_require_a_processed_selection() {
        let s = HaloState::default();
        s.begin_activation().unwrap();
        s.install_session(session("a"));
        assert_eq!(
            s.begin_request("a", "hi").unwrap_err().kind,
            ErrorKind::InvalidSelection
        );
    }

    #[test]
    fn duplicate_submission_is_rejected() {
        let s = HaloState::default();
        ready(&s, "a");
        let (r1, _) = s.begin_request("a", "q1").unwrap();
        assert_eq!(
            s.begin_request("a", "q2").unwrap_err().kind,
            ErrorKind::Busy
        );
        assert!(s.finish_request(&r1.id, Some("a1".into()), Phase::Completed));
        assert!(s.begin_request("a", "q2").is_ok());
    }

    #[test]
    fn empty_question_rejected() {
        let s = HaloState::default();
        ready(&s, "a");
        assert_eq!(
            s.begin_request("a", "   ").unwrap_err().kind,
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn stale_session_ids_are_rejected() {
        let s = HaloState::default();
        ready(&s, "a");
        assert_eq!(
            s.begin_request("old", "q").unwrap_err().kind,
            ErrorKind::StaleSession
        );
        assert!(s.frame("old").is_err());
    }

    #[test]
    fn conversation_context_accumulates_and_is_sent() {
        let s = HaloState::default();
        ready(&s, "a");
        let (r1, p1) = s.begin_request("a", "q1").unwrap();
        assert_eq!(p1.contents.len(), 1);
        s.finish_request(&r1.id, Some("a1".into()), Phase::Completed);
        let (r2, p2) = s.begin_request("a", "q2").unwrap();
        assert_eq!(p2.contents.len(), 3);
        assert_eq!(p2.contents[1].parts[0].text.as_deref(), Some("a1"));
        s.finish_request(&r2.id, Some("a2".into()), Phase::Completed);
        assert_eq!(s.history_len("a").unwrap(), 2);
    }

    #[test]
    fn failed_or_cancelled_requests_do_not_enter_history() {
        let s = HaloState::default();
        ready(&s, "a");
        let (r1, _) = s.begin_request("a", "q1").unwrap();
        assert!(s.cancel_request(&r1.id));
        assert!(r1.cancel.is_cancelled());
        s.finish_request(&r1.id, None, Phase::Cancelled);
        assert_eq!(s.history_len("a").unwrap(), 0);
        assert_eq!(s.phase(), Phase::Cancelled);
    }

    #[test]
    fn request_ids_are_isolated() {
        let s = HaloState::default();
        ready(&s, "a");
        let (r1, _) = s.begin_request("a", "q1").unwrap();
        assert!(!s.cancel_request("some-other-id"));
        assert!(!r1.cancel.is_cancelled());
        assert!(!s.finish_request("some-other-id", Some("x".into()), Phase::Completed));
        assert!(s.is_active(&r1.id));
        assert_eq!(s.history_len("a").unwrap(), 0);
    }

    #[test]
    fn late_completion_after_new_selection_is_discarded() {
        let s = HaloState::default();
        ready(&s, "a");
        let (r1, _) = s.begin_request("a", "q1").unwrap();
        // User redraws the selection while the answer is streaming.
        let ctx = s.begin_selection("a").unwrap();
        assert!(r1.cancel.is_cancelled());
        assert!(!s.is_active(&r1.id));
        assert!(!s.finish_request(&r1.id, Some("stale".into()), Phase::Completed));
        s.set_visual("a", ctx, VisualContext::new("image/png", b"y"), stats())
            .unwrap();
        assert_eq!(s.history_len("a").unwrap(), 0);
        // A visual from the previous selection context is rejected.
        assert!(
            s.set_visual("a", ctx - 1, VisualContext::new("image/png", b"z"), stats())
                .is_err()
        );
    }

    #[test]
    fn new_activation_cancels_inflight_request_and_drops_session() {
        let s = HaloState::default();
        ready(&s, "a");
        let (r1, _) = s.begin_request("a", "q1").unwrap();
        s.begin_activation().unwrap();
        assert!(r1.cancel.is_cancelled());
        assert!(!s.has_session());
        assert!(!s.finish_request(&r1.id, Some("late".into()), Phase::Completed));
    }

    #[test]
    fn dismiss_cleans_up() {
        let s = HaloState::default();
        ready(&s, "a");
        let (r1, _) = s.begin_request("a", "q1").unwrap();
        assert!(!s.end_session(Some("other")));
        assert!(s.has_session());
        assert!(s.end_session(Some("a")));
        assert!(r1.cancel.is_cancelled());
        assert!(!s.has_session());
        assert_eq!(s.phase(), Phase::Idle);
    }
}
