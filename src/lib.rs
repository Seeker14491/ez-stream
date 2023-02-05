use std::error::Error;
use std::fmt::{Debug, Display};

use futures_util::{future, Future, FutureExt, Stream, StreamExt};

#[derive(Debug, Clone)]
pub struct Sender<T> {
    tx: futures_channel::mpsc::UnboundedSender<T>,
}

impl<T> Sender<T> {
    pub fn send(&self, item: T) -> Result<(), ReceiverDroppedError<T>> {
        self.tx
            .unbounded_send(item)
            .map_err(|err| ReceiverDroppedError(err.into_inner()))
    }
}

#[derive(Debug)]
pub struct ReceiverDroppedError<T>(T);

impl<T> ReceiverDroppedError<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Display for ReceiverDroppedError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "send failed because receiver was dropped")
    }
}

impl<T: Debug> Error for ReceiverDroppedError<T> {}

pub fn unbounded<T, Fut>(producer: impl FnOnce(Sender<T>) -> Fut) -> impl Stream<Item = T>
where
    Fut: Future<Output = ()>,
{
    let (tx, rx) = futures_channel::mpsc::unbounded();
    let driver_fut = producer(Sender { tx });
    let [driver_stream, driven_stream] = [
        driver_fut.into_stream().map(|()| None).left_stream(),
        rx.map(Some).right_stream(),
    ];

    futures_util::stream::select(driver_stream, driven_stream).filter_map(future::ready)
}

pub fn try_unbounded<T, E, Fut>(
    producer: impl FnOnce(Sender<T>) -> Fut,
) -> impl Stream<Item = Result<T, E>>
where
    Fut: Future<Output = Result<(), E>>,
{
    let (tx, rx) = futures_channel::mpsc::unbounded();
    let driver_fut = producer(Sender { tx });
    let [driver_stream, driven_stream] = [
        driver_fut
            .into_stream()
            .map(|result| match result {
                Ok(()) => None,
                Err(e) => Some(Err(e)),
            })
            .left_stream(),
        rx.map(|item| Some(Ok(item))).right_stream(),
    ];

    futures_util::stream::select(driver_stream, driven_stream).filter_map(future::ready)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::StreamExt;

    use super::*;

    #[tokio::test]
    async fn test_unbounded() {
        let count_to_ten = unbounded(|tx| async move {
            for n in 0..10 {
                tx.send(n).unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        let xs: Vec<_> = count_to_ten.collect().await;

        assert_eq!(&xs, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[tokio::test]
    async fn test_unbounded_with_receiver_dropped() {
        let mut sender = None;
        let sender_ref = &mut sender;
        let count_to_ten = unbounded(|tx| async move {
            *sender_ref = Some(tx.clone());
            for n in 0..10 {
                tx.send(n).unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        let xs: Vec<_> = count_to_ten.take(5).collect().await;

        assert_eq!(&xs, &[0, 1, 2, 3, 4]);

        let result = sender.unwrap().send(20);
        assert!(matches!(result, Err(ReceiverDroppedError(20))));
    }

    #[tokio::test]
    async fn test_try_unbounded_ok() {
        let stream = try_unbounded(|tx| async move {
            for n in 0..5 {
                tx.send(n).unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            Ok::<_, &'static str>(())
        });

        let xs: Vec<_> = stream.collect().await;

        assert_eq!(&xs, &[Ok(0), Ok(1), Ok(2), Ok(3), Ok(4)]);
    }

    #[tokio::test]
    async fn test_try_unbounded_err() {
        let stream = try_unbounded(|tx| async move {
            for n in 0..5 {
                if n == 3 {
                    return Err("error msg");
                }

                tx.send(n).unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            Ok(())
        });

        let xs: Vec<_> = stream.collect().await;

        assert_eq!(&xs, &[Ok(0), Ok(1), Ok(2), Err("error msg")]);
    }
}
