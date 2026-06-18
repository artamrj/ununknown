use super::*;

pub async fn events(
    State(s): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let mut rx = s.events.subscribe();
    Sse::new(
        async_stream::stream! {while let Ok(value)=rx.recv().await{yield Ok(Event::default().json_data(value).unwrap());}},
    )
}
