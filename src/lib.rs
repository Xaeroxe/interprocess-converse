use futures_util::{SinkExt, Stream, StreamExt};
use interprocess::local_socket::ToLocalSocketName;
use interprocess_typed::{
    LocalSocketListenerTyped, LocalSocketStreamTyped, OwnedReadHalfTyped, OwnedWriteHalfTyped,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};

#[cfg(test)]
mod tests;

#[derive(Deserialize, Serialize)]
struct InternalMessage<T> {
    user_message: T,
    conversation_id: u64,
    is_reply: bool,
}

pub struct ReceivedMessage<T> {
    message: Option<T>,
    conversation_id: u64,
    raw_write: Arc<Mutex<OwnedWriteHalfTyped<InternalMessage<T>>>>,
}

impl<T: Serialize + Unpin> ReceivedMessage<T> {
    pub fn take_message(&mut self) -> T {
        self.message.take().expect("message already taken!")
    }

    pub fn take_message_opt(&mut self) -> Option<T> {
        self.message.take()
    }

    pub async fn reply(self, reply: T) -> Result<(), Error> {
        SinkExt::send(
            &mut *self.raw_write.lock().await,
            InternalMessage {
                user_message: reply,
                is_reply: true,
                conversation_id: self.conversation_id,
            },
        )
        .await
        .map_err(Into::into)
    }
}

struct ReplySender<T> {
    reply_sender: Option<oneshot::Sender<Result<T, Error>>>,
    conversation_id: u64,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Bincode(bincode::Error),
    ReceivedMessageTooLarge,
    SentMessageTooLarge,
    TimeOut,
    ReadHalfDropped,
    CannotReplyToReply,
}

impl From<interprocess_typed::Error> for Error {
    fn from(e: interprocess_typed::Error) -> Self {
        match e {
            interprocess_typed::Error::Io(e) => Error::Io(e),
            interprocess_typed::Error::Bincode(e) => Error::Bincode(e),
            interprocess_typed::Error::ReceivedMessageTooLarge => Error::ReceivedMessageTooLarge,
            interprocess_typed::Error::SentMessageTooLarge => Error::SentMessageTooLarge,
        }
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct LocalSocketListener<T> {
    raw: LocalSocketListenerTyped<InternalMessage<T>>,
}

impl<T> LocalSocketListener<T> {
    pub fn bind<'a>(name: impl ToLocalSocketName<'a>) -> io::Result<Self> {
        Ok(Self {
            raw: LocalSocketListenerTyped::bind(name)?,
        })
    }

    pub async fn accept_with_limit(&self, size_limit: u64) -> io::Result<LocalSocketStream<T>> {
        self.raw.accept_with_limit(size_limit).await.map(|raw| {
            let (read, write) = raw.into_split();
            LocalSocketStream {
                read,
                write,
                next_id: 0,
            }
        })
    }

    pub async fn accept(&self) -> io::Result<LocalSocketStream<T>> {
        self.raw.accept().await.map(|raw| {
            let (read, write) = raw.into_split();
            LocalSocketStream {
                read,
                write,
                next_id: 0,
            }
        })
    }
}

/// TODO: Docs
///
/// You may have noticed that unlike `interprocess` and `interprocess-typed`, you cannot send or receive with this until you split it
/// into its two halves. This is intentional, because due to the reply mechanism it would be too easy to accidentally dead lock your programs
/// if you were using this without splitting it.
pub struct LocalSocketStream<T> {
    read: OwnedReadHalfTyped<InternalMessage<T>>,
    write: OwnedWriteHalfTyped<InternalMessage<T>>,
    next_id: u64,
}

impl<T> LocalSocketStream<T> {
    pub async fn connect_with_limit<'a>(
        name: impl ToLocalSocketName<'a>,
        size_limit: u64,
    ) -> io::Result<Self> {
        let (read, write) = LocalSocketStreamTyped::connect_with_limit(name, size_limit)
            .await?
            .into_split();
        Ok(Self {
            read,
            write,
            next_id: 0,
        })
    }

    pub async fn connect<'a>(name: impl ToLocalSocketName<'a>) -> io::Result<Self> {
        let (read, write) = LocalSocketStreamTyped::connect(name).await?.into_split();
        Ok(Self {
            read,
            write,
            next_id: 0,
        })
    }

    pub fn into_split(self) -> (OwnedReadHalf<T>, OwnedWriteHalf<T>) {
        let (reply_data_sender, reply_data_receiver) = mpsc::unbounded_channel();
        let write = Arc::new(Mutex::new(self.write));
        let write_clone = Arc::clone(&write);
        (
            OwnedReadHalf {
                raw: self.read,
                raw_write: write,
                reply_data_receiver,
                pending_reply: vec![],
            },
            OwnedWriteHalf {
                raw: write_clone,
                next_id: self.next_id,
                reply_data_sender,
            },
        )
    }

    pub fn peer_pid(&self) -> io::Result<u32> {
        self.read.peer_pid()
    }
}

pub struct OwnedReadHalf<T> {
    raw: OwnedReadHalfTyped<InternalMessage<T>>,
    raw_write: Arc<Mutex<OwnedWriteHalfTyped<InternalMessage<T>>>>,
    reply_data_receiver: mpsc::UnboundedReceiver<ReplySender<T>>,
    pending_reply: Vec<ReplySender<T>>,
}

impl<T> OwnedReadHalf<T> {
    pub fn peer_pid(&self) -> io::Result<u32> {
        self.raw.peer_pid()
    }
}

impl<T: DeserializeOwned + Unpin + Send + 'static> OwnedReadHalf<T> {
    /// Spawns a future onto the tokio runtime that will drive the receive mechanism.
    /// This allows you to receive replies to your messages, while completely ignoring any non-reply messages you get.
    ///
    /// If instead you'd like to see the non-reply messages then you'll need to drive the `Stream` implementation
    /// for `OwnedReadHalf`.
    pub fn drive_forever(mut self) {
        tokio::spawn(async move { while StreamExt::next(&mut self).await.is_some() {} });
    }
}

impl<T: DeserializeOwned + Unpin> Stream for OwnedReadHalf<T> {
    type Item = Result<ReceivedMessage<T>, Error>;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Self {
            ref mut raw,
            ref mut reply_data_receiver,
            ref mut pending_reply,
            ref raw_write,
        } = self.get_mut();
        loop {
            return match Pin::new(&mut *raw).poll_next(cx) {
                Poll::Ready(Some(Ok(i))) => {
                    while let Ok(reply_data) = reply_data_receiver.try_recv() {
                        pending_reply.push(reply_data);
                    }
                    let start_len = pending_reply.len();
                    let mut user_message = Some(i.user_message);
                    pending_reply.retain_mut(|pending_reply| {
                        if let Some(reply_sender) = pending_reply.reply_sender.as_ref() {
                            if reply_sender.is_closed() {
                                return false;
                            }
                        }
                        let matches =
                            i.is_reply && pending_reply.conversation_id == i.conversation_id;
                        if matches {
                            let _ = pending_reply
                                .reply_sender
                                .take()
                                .expect("infallible")
                                .send(Ok(user_message.take().expect("infallible")));
                        }
                        !matches
                    });
                    if start_len == pending_reply.len() {
                        Poll::Ready(Some(Ok(ReceivedMessage {
                            message: Some(user_message.take().expect("infallible")),
                            conversation_id: i.conversation_id,
                            raw_write: Arc::clone(raw_write),
                        })))
                    } else {
                        continue;
                    }
                }
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e.into()))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

pub struct OwnedWriteHalf<T> {
    raw: Arc<Mutex<OwnedWriteHalfTyped<InternalMessage<T>>>>,
    reply_data_sender: mpsc::UnboundedSender<ReplySender<T>>,
    next_id: u64,
}

impl<T: Serialize + Unpin> OwnedWriteHalf<T> {
    pub async fn peer_pid(&self) -> io::Result<u32> {
        self.raw.lock().await.peer_pid()
    }

    /// Returns a future wrapped in a future. You must `.await` the first layer to send the message, however `.await`ing the second layer
    /// is optional. You only need to `.await` the second layer if you care about the reply to this message.
    pub async fn send(
        &mut self,
        message: T,
    ) -> Result<impl Future<Output = Result<T, Error>>, Error> {
        self.send_timeout(DEFAULT_TIMEOUT, message).await
    }

    /// Returns a future wrapped in a future. You must `.await` the first layer to send the message, however `.await`ing the second layer
    /// is optional. You only need to `.await` the second layer if you care about the reply to this message.
    pub async fn send_timeout(
        &mut self,
        timeout: Duration,
        message: T,
    ) -> Result<impl Future<Output = Result<T, Error>>, Error> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        let read_half_dropped = self
            .reply_data_sender
            .send(ReplySender {
                reply_sender: Some(reply_sender),
                conversation_id: self.next_id,
            })
            .is_err();
        SinkExt::send(
            &mut *self.raw.lock().await,
            InternalMessage {
                user_message: message,
                conversation_id: self.next_id,
                is_reply: false,
            },
        )
        .await?;
        self.next_id += 1;
        Ok(async move {
            if read_half_dropped {
                return Err(Error::ReadHalfDropped);
            }
            let res = tokio::time::timeout(timeout, reply_receiver).await;
            match res {
                Ok(Ok(Ok(value))) => Ok(value),
                Ok(Ok(Err(e))) => Err(e),
                Ok(Err(_)) => Err(Error::ReadHalfDropped),
                Err(_) => Err(Error::TimeOut),
            }
        })
    }
}