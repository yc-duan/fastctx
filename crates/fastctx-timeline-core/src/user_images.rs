use fastctx_types::session::{Happened, SessionEvent};

pub fn merge(recorded: &mut SessionEvent, hosted: &SessionEvent) {
    if let (Happened::Heard { message: Some(recorded), .. }, Happened::Heard { message: Some(hosted), .. }) =
        (&mut recorded.happened, &hosted.happened)
    {
        recorded.images = hosted.images.clone();
        recorded.content = hosted.content.clone();
        recorded.detail = hosted.detail.clone();
        recorded.presentation = hosted.presentation.clone();
        recorded.hook_fragments = hosted.hook_fragments.clone();
        recorded.client_id = hosted.client_id.clone();
    }
}
