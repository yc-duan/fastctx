use std::path::PathBuf;

pub async fn choose(name: String) -> Result<Option<PathBuf>, String> {
    let result = async {
        let files = ashpd::desktop::file_chooser::SelectedFiles::save_file()
            .current_name(name.as_str())
            .modal(true)
            .send()
            .await?
            .response()?;
        if files.uris().len() != 1 {
            return Err(ashpd::Error::ParseError("the save dialog returned no single destination"));
        }
        files.uris()[0]
            .to_file_path()
            .map(Some)
            .map_err(|_| ashpd::Error::ParseError("the save destination is not a local file"))
    }
    .await;
    match result {
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(None),
        result => result.map_err(|error| error.to_string()),
    }
}
