use crate::{Error, Notification};
use futures::future;
use futures::{pin_mut, Stream};
use log::info;
use std::collections::VecDeque;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::runtime::Runtime;
use tokio_postgres::AsyncMessage;

pub struct Connection {
    runtime: Runtime,
    connection: Pin<Box<dyn Stream<Item = Result<AsyncMessage, Error>> + Send>>,
    notifications: VecDeque<Notification>,
}

impl Connection {
    pub fn new<S, T>(runtime: Runtime, connection: tokio_postgres::Connection<S, T>) -> Connection
    where
        S: AsyncRead + AsyncWrite + Unpin + 'static + Send,
        T: AsyncRead + AsyncWrite + Unpin + 'static + Send,
    {
        Connection {
            runtime,
            connection: Box::pin(ConnectionStream { connection }),
            notifications: VecDeque::new(),
        }
    }

    pub fn as_ref(&mut self) -> ConnectionRef<'_> {
        ConnectionRef { connection: self }
    }

    pub fn enter<F, T>(&self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        self.runtime.enter(f)
    }

    pub fn block_on<F, T>(&mut self, future: F) -> Result<T, Error>
    where
        F: Future<Output = Result<T, Error>>,
    {
        pin_mut!(future);
        self.poll_block_on(|cx, _, _| future.as_mut().poll(cx))
    }

    pub fn poll_block_on<F, T>(&mut self, mut f: F) -> Result<T, Error>
    where
        F: FnMut(&mut Context<'_>, &mut VecDeque<Notification>, bool) -> Poll<Result<T, Error>>,
    {
        let connection = &mut self.connection;
        let notifications = &mut self.notifications;
        self.runtime.block_on({
            future::poll_fn(|cx| {
                let done = loop {
                    match connection.as_mut().poll_next(cx) {
                        Poll::Ready(Some(Ok(AsyncMessage::Notification(notification)))) => {
                            notifications.push_back(notification);
                        }
                        Poll::Ready(Some(Ok(AsyncMessage::Notice(notice)))) => {
                            info!("{}: {}", notice.severity(), notice.message());
                        }
                        Poll::Ready(Some(Ok(_))) => {}
                        Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                        Poll::Ready(None) => break true,
                        Poll::Pending => break false,
                    }
                };

                f(cx, notifications, done)
            })
        })
    }

    pub fn notifications(&self) -> &VecDeque<Notification> {
        &self.notifications
    }

    pub fn notifications_mut(&mut self) -> &mut VecDeque<Notification> {
        &mut self.notifications
    }
}

pub struct ConnectionRef<'a> {
    connection: &'a mut Connection,
}

// no-op impl to extend the borrow until drop
impl Drop for ConnectionRef<'_> {
    #[inline]
    fn drop(&mut self) {}
}

impl Deref for ConnectionRef<'_> {
    type Target = Connection;

    #[inline]
    fn deref(&self) -> &Connection {
        self.connection
    }
}

impl DerefMut for ConnectionRef<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection
    }
}

struct ConnectionStream<S, T> {
    connection: tokio_postgres::Connection<S, T>,
}

impl<S, T> Stream for ConnectionStream<S, T>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Item = Result<AsyncMessage, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.connection.poll_message(cx)
    }
}
