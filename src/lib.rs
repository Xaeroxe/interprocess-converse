#![doc = include_str!("../README.md")]

use async_io_converse::{AsyncReadConverse, AsyncWriteConverse, new_duplex_connection_with_limit, new_duplex_connection};
use futures_util::{Stream, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use interprocess::local_socket::tokio::{OwnedWriteHalf as InterprocessOwnedWriteHalf, OwnedReadHalf as InterprocessOwnedReadHalf, LocalSocketListener as InterprocessLocalSocketListener, LocalSocketStream as InterprocessLocalSocketStream};
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration, marker::PhantomData, ffi::OsString,
};

#[cfg(test)]
mod tests;

pub type ReceivedMessage<T> = async_io_converse::ReceivedMessage<InterprocessOwnedWriteHalf, T>;

/// Errors which can occur on an `interprocess-converse` connection.
#[derive(Debug)]
pub enum Error {
    /// Error from `std::io`
    Io(io::Error),
    /// Error from the `bincode` crate
    Bincode(bincode::Error),
    /// A message was received that exceeded the configured length limit
    ReceivedMessageTooLarge,
    /// A message was sent that exceeded the configured length limit
    SentMessageTooLarge,
    /// A reply wasn't received within the timeout specified
    Timeout,
    /// The read half was dropped, crippling the ability to receive replies.
    ReadHalfDropped,
}

impl From<async_io_converse::Error> for Error {
    fn from(e: async_io_converse::Error) -> Self {
        match e {
            async_io_converse::Error::Io(e) => Error::Io(e),
            async_io_converse::Error::Bincode(e) => Error::Bincode(e),
            async_io_converse::Error::ReceivedMessageTooLarge => Error::ReceivedMessageTooLarge,
            async_io_converse::Error::SentMessageTooLarge => Error::SentMessageTooLarge,
            async_io_converse::Error::Timeout => Error::Timeout,
            async_io_converse::Error::ReadHalfDropped => Error::ReadHalfDropped,
        }
    }
}

pub use interprocess::local_socket::ToLocalSocketName;

/// Listens for new connections on the bound socket name.
pub struct LocalSocketListener<T: Serialize + DeserializeOwned + Unpin> {
    raw: InterprocessLocalSocketListener,
    _phantom: PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned + Unpin> LocalSocketListener<T> {
    /// Begins listening for connections to the given socket name. The socket does not need to exist prior to calling this function.
    pub fn bind<'a>(name: impl ToLocalSocketName<'a>) -> io::Result<Self> {
        Ok(Self {
            raw: InterprocessLocalSocketListener::bind(name)?,
            _phantom: PhantomData,
        })
    }

    /// Accepts the connection, initializing it with the given size limit specified in bytes.
    ///
    /// Be careful, large limits might create a vulnerability to a Denial of Service attack.
    pub async fn accept_with_limit(&self, size_limit: u64) -> io::Result<LocalSocketStream<T>> {
        self.raw.accept().await.map(|raw| {
            let (read, write) = raw.into_split();
            let (read, write) = new_duplex_connection_with_limit(size_limit, read, write);
            LocalSocketStream {
                read,
                write,
            }
        })
    }

    /// Accepts the connection, initializing it with a default size limit of 1 MB per message.
    pub async fn accept(&self) -> io::Result<LocalSocketStream<T>> {
        self.raw.accept().await.map(|raw| {
            let (read, write) = raw.into_split();
            let (read, write) = new_duplex_connection(read, write);
            LocalSocketStream {
                read,
                write,
            }
        })
    }
}

/// A duplex async connection for sending and receiving messages of a particular type, and optionally replying to those messages.
///
/// You may have noticed that unlike `interprocess` and `interprocess-typed`, you cannot send or receive with this until you split it
/// into its two halves. This is intentional, because due to the reply mechanism it would be too easy to accidentally dead lock your programs
/// if you were using this without splitting it.
pub struct LocalSocketStream<T: Serialize + DeserializeOwned + Unpin> {
    read: AsyncReadConverse<InterprocessOwnedReadHalf, InterprocessOwnedWriteHalf, T>,
    write: AsyncWriteConverse<InterprocessOwnedWriteHalf, T>,
}

impl<T: Serialize + DeserializeOwned + Unpin> LocalSocketStream<T> {
    /// Creates a connection, initializing it with the given size limit specified in bytes.
    ///
    /// Be careful, large limits might create a vulnerability to a Denial of Service attack.
    pub async fn connect_with_limit<'a>(
        name: impl ToLocalSocketName<'a>,
        size_limit: u64,
    ) -> io::Result<Self> {
        let (read, write) = InterprocessLocalSocketStream::connect(name)
            .await?
            .into_split();
        let (read, write) = new_duplex_connection_with_limit(size_limit, read, write);
        Ok(Self {
            read,
            write,
        })
    }

    /// Creates a connection, initializing it with a default size limit of 1 MB per message.
    pub async fn connect<'a>(name: impl ToLocalSocketName<'a>) -> io::Result<Self> {
        let (read, write) = InterprocessLocalSocketStream::connect(name).await?.into_split();
        let (read, write) = new_duplex_connection(read, write);
        Ok(Self {
            read,
            write,
        })
    }

    /// Splits this into two parts, the first can be used for reading from the socket, the second can be used for writing to the socket.
    pub fn into_split(self) -> (OwnedReadHalf<T>, OwnedWriteHalf<T>) {
        (
            OwnedReadHalf {
                raw: self.read,
            },
            OwnedWriteHalf {
                raw: self.write,
            },
        )
    }

    /// Returns the process id of the connected peer.
    pub fn peer_pid(&self) -> io::Result<u32> {
        self.read.inner().peer_pid()
    }
}

/// Used to receive messages from the connected peer. ***You must drive this in order to receive replies on the [OwnedWriteHalf]***
pub struct OwnedReadHalf<T: Serialize + DeserializeOwned + Unpin> {
    raw: AsyncReadConverse<InterprocessOwnedReadHalf, InterprocessOwnedWriteHalf, T>,
}

impl<T: Serialize + DeserializeOwned + Unpin> OwnedReadHalf<T> {
    /// Returns the process id of the connected peer.
    pub fn peer_pid(&self) -> io::Result<u32> {
        self.raw.inner().peer_pid()
    }
}

impl<T: Serialize + DeserializeOwned + Unpin + Send + 'static> OwnedReadHalf<T> {
    /// Spawns a future onto the tokio runtime that will drive the receive mechanism.
    /// This allows you to receive replies to your messages, while completely ignoring any non-reply messages you get.
    ///
    /// If instead you'd like to see the non-reply messages then you'll need to drive the `Stream` implementation
    /// for `OwnedReadHalf`.
    pub fn drive_forever(mut self) {
        tokio::spawn(async move { while StreamExt::next(&mut self).await.is_some() {} });
    }
}

impl<T: Serialize + DeserializeOwned + Unpin> Stream for OwnedReadHalf<T> {
    type Item = Result<ReceivedMessage<T>, Error>;

    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.raw).poll_next(cx).map_err(Into::into)
    }
}

/// Used to send messages to the connected peer. You may optionally receive replies to your messages as well.
///
/// ***You must drive the corresponding [OwnedReadHalf] in order to receive replies to your messages.***
/// You can do this either by driving the `Stream` implementation, or calling [OwnedReadHalf::drive_forever].
pub struct OwnedWriteHalf<T: Serialize + DeserializeOwned + Unpin> {
    raw: AsyncWriteConverse<InterprocessOwnedWriteHalf, T>,
}

impl<T: Serialize + DeserializeOwned + Unpin> OwnedWriteHalf<T> {
    /// Returns the process id of the connected peer.
    pub async fn peer_pid(&self) -> io::Result<u32> {
        self.raw.with_inner(|raw| raw.peer_pid()).await
    }

    /// Send a message, and wait for a reply, with the default timeout. Shorthand for `.await`ing both layers of `.send(message)`.
    pub async fn ask(&mut self, message: T) -> Result<T, Error> {
        self.raw.ask(message).await.map_err(Into::into)
    }

    /// Send a message, and wait for a reply, up to timeout. Shorthand for `.await`ing both layers of `.send_timeout(message)`.
    pub async fn ask_timeout(&mut self, timeout: Duration, message: T) -> Result<T, Error> {
        self.raw.ask_timeout(timeout, message).await.map_err(Into::into)
    }

    /// Sends a message to the peer on the other side of the connection. This returns a future wrapped in a future. You must
    /// `.await` the first layer to send the message, however `.await`ing the second layer is optional. You only need to
    /// `.await` the second layer if you care about the reply to your message. Waits up to the default timeout for a reply.
    pub async fn send(
        &mut self,
        message: T,
    ) -> Result<impl Future<Output = Result<T, Error>>, Error> {
        self.raw.send(message).await.map_err(Into::into).map(|f| async move {
            f.await.map_err(Into::into)
        })
    }

    /// Sends a message to the peer on the other side of the connection, waiting up to the specified timeout for a reply.
    /// This returns a future wrapped in a future. You must  `.await` the first layer to send the message, however
    /// `.await`ing the second layer is optional. You only need to  `.await` the second layer if you care about the
    /// reply to your message.
    pub async fn send_timeout(
        &mut self,
        timeout: Duration,
        message: T,
    ) -> Result<impl Future<Output = Result<T, Error>>, Error> {
        self.raw.send_timeout(timeout, message).await.map_err(Into::into).map(|f| async move {
            f.await.map_err(Into::into)
        })
    }
}

/// Randomly generates a socket name suitable for the operating system in use.
pub fn generate_socket_name() -> io::Result<OsString> {
    #[cfg(windows)]
    {
        Ok(OsString::from(format!("@{}.sock", uuid::Uuid::new_v4())))
    }
    #[cfg(not(windows))]
    {
        let path = tempfile::tempdir()?.into_path().join("ipc_socket");
        Ok(path.into_os_string())
    }
}