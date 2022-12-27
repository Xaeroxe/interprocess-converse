use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Instant,
};

use futures_util::StreamExt;
use interprocess_typed::generate_socket_name;

use super::*;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum TestMessage {
    HelloThere,
    GeneralKenobiYouAreABoldOne,
}

#[tokio::test]
async fn basic_dialogue() {
    let socket_name = generate_socket_name().unwrap();
    let listener = LocalSocketListener::bind(socket_name.as_os_str()).unwrap();
    let client = LocalSocketStream::connect(socket_name.as_os_str())
        .await
        .unwrap();
    let server = listener.accept().await.unwrap();
    let (server_read, mut server_write) = server.into_split();
    let (mut client_read, _client_write) = client.into_split();
    server_read.drive_forever();
    tokio::spawn(async move {
        while let Some(message) = client_read.next().await {
            let mut received_message = message.unwrap();
            let message = received_message.take_message();
            match message {
                TestMessage::HelloThere => received_message
                    .reply(TestMessage::GeneralKenobiYouAreABoldOne)
                    .await
                    .unwrap(),
                TestMessage::GeneralKenobiYouAreABoldOne => panic!("Wait, that's my line!"),
            }
        }
    });
    assert_eq!(
        server_write
            .send(TestMessage::HelloThere)
            .await
            .unwrap()
            .await
            .unwrap(),
        TestMessage::GeneralKenobiYouAreABoldOne
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn many_clients() {
    let socket_name = generate_socket_name().unwrap();
    let listener = LocalSocketListener::bind(socket_name.as_os_str()).unwrap();
    const TEST_DURATION: Duration = Duration::from_secs(1);
    const TEST_COUNT: u32 = 5;
    let tests_complete = Arc::new(AtomicU32::new(0));
    let start = Instant::now();
    for _ in 0..TEST_COUNT {
        let client = LocalSocketStream::connect(socket_name.as_os_str())
            .await
            .unwrap();
        let server = listener.accept().await.unwrap();
        let tests_complete = Arc::clone(&tests_complete);
        tokio::spawn(async move {
            let (server_read, mut server_write) = server.into_split();
            let (mut client_read, _client_write) = client.into_split();
            server_read.drive_forever();
            tokio::spawn(async move {
                while let Some(message) = client_read.next().await {
                    let mut received_message = message.unwrap();
                    let message = received_message.take_message();
                    match message {
                        TestMessage::HelloThere => received_message
                            .reply(TestMessage::GeneralKenobiYouAreABoldOne)
                            .await
                            .unwrap(),
                        TestMessage::GeneralKenobiYouAreABoldOne => panic!("Wait, that's my line!"),
                    }
                }
            });

            let start = Instant::now();
            while start.elapsed() < TEST_DURATION {
                assert_eq!(
                    server_write
                        .send(TestMessage::HelloThere)
                        .await
                        .unwrap()
                        .await
                        .unwrap(),
                    TestMessage::GeneralKenobiYouAreABoldOne
                );
            }
            tests_complete.fetch_add(1, Ordering::Relaxed);
        });
    }
    while tests_complete.load(Ordering::Relaxed) < TEST_COUNT {}
    assert!(start.elapsed() >= TEST_DURATION);
    assert!(start.elapsed() < TEST_DURATION * 2);
}
