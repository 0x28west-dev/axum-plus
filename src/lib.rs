use std::{
    any::type_name,
    task::{Context, Poll},
};

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, Request, StatusCode},
};
use std_plus::new;
use tower_layer::Layer;
use tower_service::Service;

#[macro_export]
macro_rules! static_s {
    ($data:expr) => {{
        $crate::StaticLayer::new($data)
    }};
}

#[derive(new, Clone)]
pub struct AddStatic<S, T: 'static> {
    inner: S,
    ext: &'static T,
}

#[derive(new, Clone)]
pub struct StaticLayer<T: 'static> {
    ext: &'static T,
}

impl<S, T> Layer<S> for StaticLayer<T>
where
    T: Clone + 'static,
{
    type Service = AddStatic<S, T>;

    fn layer(&self, inner: S) -> Self::Service {
        AddStatic::new(inner, self.ext)
    }
}

#[derive(new, Clone)]
pub struct Static<T: 'static>(pub &'static T);

impl<T> std::ops::Deref for Static<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<ReqBody, S, T> Service<Request<ReqBody>> for AddStatic<S, T>
where
    S: Service<Request<ReqBody>>,
    Static<T>: Send + Sync + Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        req.extensions_mut().insert(Static::new(self.ext));
        self.inner.call(req)
    }
}

#[async_trait::async_trait]
impl<S, T> FromRequestParts<S> for Static<T>
where
    Static<T>: Send + Send + Sync + 'static + Clone,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        if let Some(value) = parts.extensions.get::<Static<T>>().cloned() {
            return Ok(value);
        }

        if cfg!(test) {
            panic!(
                "Failed to  extract {}, is it added via StaticLayer",
                type_name::<Static<T>>()
            )
        } else {
            tracing::error!(
                "Failed to  extract {}, is it added via StaticLayer",
                type_name::<Static<T>>()
            );
        }

        Err((StatusCode::INTERNAL_SERVER_ERROR, "Unknown error occurred!"))
    }
}

#[cfg(test)]
mod test {
    use crate::{static_s, Static};
    use anyhow::{anyhow, Result};
    use axum::http::{Request, Response};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use std::any::type_name;
    use std::sync::LazyLock;
    use std_plus::{f, lazy_lock, new, to_static, Encoding as _, Standard, B64};
    use tower::BoxError;
    use tower::{service_fn, ServiceBuilder, ServiceExt};

    type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, BoxError>;

    #[allow(dead_code)]
    pub struct Body(BoxBody);

    impl Body {
        pub fn new<B>(body: B) -> Self
        where
            B: http_body::Body<Data = Bytes> + Send + 'static,
            B::Error: Into<BoxError>,
        {
            Self(body.map_err(Into::into).boxed_unsync())
        }

        pub fn empty() -> Self {
            Self::new(http_body_util::Empty::new())
        }
    }

    #[derive(Debug, new, Clone)]
    struct Data(&'static str);

    #[tokio::test]
    async fn static_service() -> Result<()> {
        async fn handler(req: Request<Body>) -> Result<Response<String>> {
            fn extractor<T>(req: &Request<Body>) -> Result<&'static T>
            where
                T: Send + Sync + 'static,
            {
                let error = anyhow!(f!("Failed to extract: {}", type_name::<T>()));
                let Static(_ext) = req.extensions().get::<Static<T>>().ok_or(error)?;

                Ok(*_ext)
            }

            let Data(data) = extractor::<Data>(&req)?;
            let encoder = extractor::<B64<Standard>>(&req)?;
            Ok(Response::new(encoder.encode(data).unwrap()))
        }

        static ENCODER: LazyLock<B64<Standard>> = lazy_lock!(B64::<Standard>::new());
        let data: &'static Data = to_static!(Data, Data::new("West"));

        let res = ServiceBuilder::new()
            .layer(static_s!(data))
            .layer(static_s!(&*ENCODER))
            .service(service_fn(handler))
            .oneshot(Request::new(Body::empty()))
            .await?
            .into_body();

        println!("{:?}", res);

        assert_eq!("West", ENCODER.decode(res).unwrap());
        Ok(())
    }
}
