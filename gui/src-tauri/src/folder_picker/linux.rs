use std::path::PathBuf;

pub async fn pick() -> Result<Option<PathBuf>, String> {
    let result = async {
        let files = ashpd::desktop::file_chooser::SelectedFiles::open_file()
            .directory(true)
            .modal(true)
            .send()
            .await?
            .response()?;
        if files.uris().len() != 1 {
            return Err(ashpd::Error::ParseError("the folder dialog returned no single folder"));
        }
        files.uris()[0]
            .to_file_path()
            .map(Some)
            .map_err(|_| ashpd::Error::ParseError("the chosen folder is not a local folder"))
    }
    .await;
    match result {
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(None),
        result => result.map_err(|error| error.to_string()),
    }
}
