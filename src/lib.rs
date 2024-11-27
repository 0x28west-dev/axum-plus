use std::{
    any::type_name,
    borrow::Cow,
    collections::HashMap,
    task::{Context, Poll},
};

use axum::{
    extract::{FromRequest, FromRequestParts, Request},
    http::{request::Parts, StatusCode},
    Json,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std_plus::{new, string};
use tower_layer::Layer;
use tower_service::Service;
use validator::{Validate, ValidationError, ValidationErrors};

macro_rules! create_status_code {
    ($($ident:ident),*) => {
        $(
            pub const $ident: axum::http::StatusCode = axum::http::StatusCode::$ident;
        )*
    };
}

create_status_code!(CONTINUE, SWITCHING_PROTOCOLS, PROCESSING);
create_status_code!(
    OK,
    CREATED,
    ACCEPTED,
    NON_AUTHORITATIVE_INFORMATION,
    NO_CONTENT,
    RESET_CONTENT,
    PARTIAL_CONTENT,
    MULTI_STATUS,
    ALREADY_REPORTED,
    IM_USED
);
create_status_code!(
    MULTIPLE_CHOICES,
    MOVED_PERMANENTLY,
    FOUND,
    SEE_OTHER,
    NOT_MODIFIED,
    USE_PROXY,
    TEMPORARY_REDIRECT,
    PERMANENT_REDIRECT
);
create_status_code!(
    BAD_REQUEST,
    UNAUTHORIZED,
    PAYMENT_REQUIRED,
    FORBIDDEN,
    NOT_FOUND,
    METHOD_NOT_ALLOWED,
    NOT_ACCEPTABLE,
    PROXY_AUTHENTICATION_REQUIRED,
    REQUEST_TIMEOUT,
    CONFLICT,
    GONE,
    LENGTH_REQUIRED,
    PRECONDITION_FAILED,
    PAYLOAD_TOO_LARGE,
    URI_TOO_LONG,
    UNSUPPORTED_MEDIA_TYPE,
    RANGE_NOT_SATISFIABLE,
    EXPECTATION_FAILED,
    IM_A_TEAPOT,
    MISDIRECTED_REQUEST,
    UNPROCESSABLE_ENTITY,
    LOCKED,
    FAILED_DEPENDENCY,
    UPGRADE_REQUIRED,
    PRECONDITION_REQUIRED,
    TOO_MANY_REQUESTS,
    REQUEST_HEADER_FIELDS_TOO_LARGE,
    UNAVAILABLE_FOR_LEGAL_REASONS
);
create_status_code!(
    INTERNAL_SERVER_ERROR,
    NOT_IMPLEMENTED,
    BAD_GATEWAY,
    SERVICE_UNAVAILABLE,
    GATEWAY_TIMEOUT,
    HTTP_VERSION_NOT_SUPPORTED,
    VARIANT_ALSO_NEGOTIATES,
    INSUFFICIENT_STORAGE,
    LOOP_DETECTED,
    NOT_EXTENDED,
    NETWORK_AUTHENTICATION_REQUIRED
);
#[derive(Deserialize)]
pub struct Body<T>(pub T);

type Store = HashMap<Cow<'static, str>, Vec<(Cow<'static, str>, Cow<'static, str>)>>;

#[derive(new, Debug, Serialize)]
pub struct Error {
    reason: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    messages: Option<Store>,
}

#[async_trait::async_trait]
impl<S, T> FromRequest<S> for Body<T>
where
    S: Send + Sync,
    T: Send + Sync + DeserializeOwned + Validate,
{
    type Rejection = (StatusCode, Json<Error>);

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(Body(body)) = Json::<Body<T>>::from_request(req, state)
            .await
            .map_err(|_| {
                let error = Error::new(string!("Failed to parsed the body into valid json!"), None);
                (BAD_REQUEST, Json(error))
            })?;

        if let Err(err) = body.validate() {
            let mut store = HashMap::new();
            make_error(None, &err, &mut store);
            let mut error = Error::new(string!("Invalid payload data!"), None);

            if !store.is_empty() {
                error.messages = Some(store)
            };

            return Err((BAD_REQUEST, Json(error)));
        };

        Ok(Body(body))
    }
}

fn make_error(key: Option<&str>, err: &ValidationErrors, store: &mut Store) {
    if err.is_empty() {
        return;
    };

    use validator::ValidationErrorsKind::*;

    fn compute_err(err: &ValidationError, column_key: &str, store_key: &str, store: &mut Store) {
        if let Some(ref message) = err.message {
            if let Some(message_store) = store.get_mut(store_key) {
                message_store.push((Cow::Owned(column_key.to_string()), message.clone()));
                return;
            }
            // Key didn't exist create the vec and populate it
            store.insert(
                Cow::Owned(store_key.to_string()),
                vec![(Cow::Owned(column_key.to_string()), message.clone())],
            );
        }
    }

    for (column_key, value) in err.0.iter() {
        match value {
            Field(fields) => {
                for field in fields {
                    compute_err(field, column_key, key.unwrap_or(&column_key), store)
                }
            }
            Struct(errors) => make_error(Some(&column_key), &**errors, store),
            List(errors) => {
                for (_, error) in errors {
                    make_error(Some(&column_key), &**error, store);
                }
            }
        }
    }
}

#[macro_export]
macro_rules! static_service {
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
    use crate::{static_service, Static};
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
            .layer(static_service!(data))
            .layer(static_service!(&*ENCODER))
            .service(service_fn(handler))
            .oneshot(Request::new(Body::empty()))
            .await?
            .into_body();

        println!("{:?}", res);

        assert_eq!("West", ENCODER.decode(res).unwrap());
        Ok(())
    }
}
