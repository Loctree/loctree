//! Fixture reproducing the CodeScribe `utterance_id` failure class.
//!
//! `utterance_id` is a *local* variable initialized and incremented deep
//! inside a large function body — plus emitted as a struct field. AST/tagmap
//! symbol search omits these local sites; the literal occurrence scanner must
//! find all of them.

/// A pipeline event carrying the running utterance counter.
pub struct EngineEvent {
    pub utterance_id: u64,
    pub text: String,
}

/// Final event shape used by the W2-B classifier acceptance fixture.
pub struct UtteranceFinal {
    pub utterance_id: u64,
    pub text: String,
}

/// Deliberately large function so the local `utterance_id` lives far from any
/// exported boundary. Mirrors a real STT engine loop.
pub fn run_pipeline(frames: &[&str]) -> Vec<EngineEvent> {
    let mut events = Vec::new();

    // Local init — invisible to symbol-only `find`.
    let mut utterance_id: u64 = 0;

    let mut accumulator = String::new();
    let mut silence_run = 0usize;
    let mut total_frames = 0usize;

    for frame in frames {
        total_frames += 1;

        if frame.is_empty() {
            silence_run += 1;

            // On a silence boundary we flush the current utterance.
            if silence_run >= 2 && !accumulator.is_empty() {
                // Increment the local counter — second occurrence site.
                utterance_id += 1;

                // Struct field emission — third occurrence site.
                events.push(EngineEvent {
                    utterance_id,
                    text: std::mem::take(&mut accumulator),
                });

                silence_run = 0;
            }

            continue;
        }

        silence_run = 0;
        if !accumulator.is_empty() {
            accumulator.push(' ');
        }
        accumulator.push_str(frame);
    }

    // Final flush for any trailing speech.
    if !accumulator.is_empty() {
        utterance_id += 1;
        events.push(EngineEvent {
            utterance_id,
            text: accumulator,
        });
    }

    let _ = total_frames;
    events
}

pub fn count(events: &[EngineEvent]) -> usize {
    events.len()
}

/// Single-line struct-literal emission so the local W2-B classifier can see the
/// enclosing `{` on the *same* line. The multiline `events.push(EngineEvent {
/// utterance_id, ... })` above keeps its shorthand sites `unknown` on purpose —
/// the opening brace is a line up, and the classifier never guesses past the
/// current line.
pub fn final_event(utterance_id: u64, text: String) -> UtteranceFinal {
    UtteranceFinal { utterance_id, text }
}
