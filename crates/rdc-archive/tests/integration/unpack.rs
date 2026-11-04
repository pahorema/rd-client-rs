use rdc_archive::Error;

#[test]
fn failure_messages_name_the_link_and_the_torrent() {
    let message = Error::Cancelled.unpack_failure_message(Some("https://host/file.rar"), "Show");

    assert_eq!(
        message,
        "An unexpected error occurred unpacking https://host/file.rar for torrent Show: \
         the unpack was cancelled"
    );
}
