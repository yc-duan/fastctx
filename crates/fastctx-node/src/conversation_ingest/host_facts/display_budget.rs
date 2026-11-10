//! Reserve the public facts before finalizing summaries and prose previews.
use fastctx_sync::store::Error;
use fastctx_types::sync::conversation_timeline::{ItemDisplayV1, display_preview};

pub fn fit(display: &mut ItemDisplayV1) -> Result<(), Error> {
    loop {
        let error = match display.encode() {
            Ok(_) => return Ok(()),
            Err(error) => error,
        };
        let mut summaries: Vec<&mut String> =
            display.parts.iter_mut().filter_map(|part| part.preview.as_mut()).collect();
        if let Some(tool) = &mut display.tool {
            summaries.push(&mut tool.name);
            summaries.extend(tool.server.iter_mut());
            summaries.extend(tool.machine.iter_mut());
            summaries.extend(tool.does.text.iter_mut());
            summaries.extend(tool.does.command.iter_mut());
            summaries.extend(tool.does.path.iter_mut());
        }
        let Some(text) = summaries.into_iter().filter(|text| text.len() > 8).max_by_key(|text| text.len()) else {
            return Err(Error::Invalid(error));
        };
        *text = display_preview(text, text.len() / 2);
    }
}
