use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};

use futures_util::StreamExt;
use rand::Rng;

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
        server_write.ask(TestMessage::HelloThere).await.unwrap(),
        TestMessage::GeneralKenobiYouAreABoldOne
    );
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum IdentifiableMessage {
    FromServer(u32),
    FromClient(u32),
}

#[tokio::test(flavor = "multi_thread")]
async fn flurry_of_communication() {
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
            let (mut server_read, mut server_write) = server.into_split();
            let (mut client_read, mut client_write) = client.into_split();
            tokio::spawn(async move {
                while let Some(message) = client_read.next().await {
                    let mut received_message = message.unwrap();
                    let message = received_message.take_message();
                    match message {
                        IdentifiableMessage::FromServer(u) => received_message
                            .reply(IdentifiableMessage::FromClient(u))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromClient(_) => panic!(
                            "Received message from client as client, this should never happen"
                        ),
                    }
                }
            });
            tokio::spawn(async move {
                while let Some(message) = server_read.next().await {
                    let mut received_message = message.unwrap();
                    let message = received_message.take_message();
                    match message {
                        IdentifiableMessage::FromClient(u) => received_message
                            .reply(IdentifiableMessage::FromServer(u))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromServer(_) => panic!(
                            "Received message from server as server, this should never happen"
                        ),
                    }
                }
            });
            let start = Instant::now();
            while start.elapsed() < TEST_DURATION {
                let code = rand::thread_rng().gen::<u32>();
                if rand::thread_rng().gen::<bool>() {
                    assert_eq!(
                        server_write
                            .ask(IdentifiableMessage::FromServer(code))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromClient(code)
                    );
                } else {
                    assert_eq!(
                        client_write
                            .ask(IdentifiableMessage::FromClient(code))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromServer(code)
                    );
                }
            }
            tests_complete.fetch_add(1, Ordering::Relaxed);
        });
    }
    while tests_complete.load(Ordering::Relaxed) < TEST_COUNT && start.elapsed() < TEST_DURATION * 2
    {
    }
    assert!(start.elapsed() >= TEST_DURATION);
    assert!(start.elapsed() < TEST_DURATION * 2);
}

#[tokio::test]
async fn timeout_check() {
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
                TestMessage::GeneralKenobiYouAreABoldOne => {
                    // Do nothing.
                }
            }
        }
    });
    let start = Instant::now();
    let timeout = Duration::from_secs(1);
    assert!(matches!(
        server_write
            .ask_timeout(timeout, TestMessage::GeneralKenobiYouAreABoldOne)
            .await,
        Err(Error::Timeout)
    ));
    let elapsed = start.elapsed();
    assert!(elapsed < timeout * 2);
    assert!(elapsed >= timeout);
    assert!(matches!(
        server_write
            .ask_timeout(timeout, TestMessage::HelloThere)
            .await,
        Ok(TestMessage::GeneralKenobiYouAreABoldOne)
    ));
}
