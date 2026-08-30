use fastctx::ToolContent;
use fastctx::read_tool::{ReadRequest, read_file};

#[test]
fn a_drive_shaped_file_uri_cannot_discard_its_remote_authority() {
    let response = read_file(ReadRequest {
        file_path: "file://example.com/C:/secret.txt".to_string(),
        offset: None,
        limit: None,
        pages: None,
        pdf_mode: None,
        encoding: None,
        view: None,
    });

    assert!(response.is_error);
    assert_eq!(
        response.content,
        vec![ToolContent::Text(
            "Invalid local file URI: remote authorities are not supported.".to_string()
        )]
    );
}
