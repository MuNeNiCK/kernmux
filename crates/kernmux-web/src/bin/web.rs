#![cfg_attr(target_arch = "wasm32", no_main)]

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::{cell::OnceCell, rc::Rc};

    use gloo_timers::future::TimeoutFuture;
    use kernmux_api::v1::{
        HostSnapshot, ImageArtifact, Instance, InstanceLifecycleMutation, Operation,
        OperationState, Response as ApiResponse, StopInstanceMutation,
    };
    use kernmux_ui_model::{Intent, ManagementSnapshot};
    use serde::{Serialize, de::DeserializeOwned};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{Event, Headers, MediaQueryList, Request, RequestInit, Response};

    type ReducedMotionListener = (MediaQueryList, Closure<dyn FnMut(Event)>);

    thread_local! {
        static APPLICATION: OnceCell<gpui::ApplicationHandle> = const { OnceCell::new() };
        static REDUCED_MOTION: OnceCell<ReducedMotionListener> = const { OnceCell::new() };
        static TOKEN: OnceCell<String> = const { OnceCell::new() };
    }

    #[wasm_bindgen::prelude::wasm_bindgen(start)]
    pub fn start() {
        gpui_platform::web_init();
        let application =
            gpui_platform::application().run_embedded(kernmux_web::open_management_shell);
        APPLICATION.with(|slot| {
            assert!(slot.set(application).is_ok(), "application already started");
        });
        kernmux_web::set_intent_handler(Rc::new(|intent| {
            spawn_local(async move {
                if let Err(message) = execute_intent(intent).await {
                    fail_shell(message);
                }
            });
        }));
        start_reduced_motion_preference();
        spawn_local(async {
            match initialize().await {
                Ok(snapshot) => install_snapshot(snapshot),
                Err(message) => fail_shell(message),
            }
        });
    }

    fn start_reduced_motion_preference() {
        let Some(query) = web_sys::window().and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        }) else {
            return;
        };
        apply_reduced_motion(query.matches());
        let observed = query.clone();
        let listener = Closure::new(move |_event: Event| apply_reduced_motion(observed.matches()));
        if query
            .add_event_listener_with_callback("change", listener.as_ref().unchecked_ref())
            .is_ok()
        {
            REDUCED_MOTION.with(|slot| {
                let _ = slot.set((query, listener));
            });
        }
    }

    fn apply_reduced_motion(reduce: bool) {
        APPLICATION.with(|slot| {
            if let Some(application) = slot.get() {
                application.update(|cx| cx.set_reduce_motion(reduce));
            }
        });
    }

    async fn initialize() -> Result<ManagementSnapshot, String> {
        let token = consume_fragment_token()?;
        TOKEN.with(|slot| {
            slot.set(token)
                .map_err(|_| "management credential is already initialized".to_owned())
        })?;
        load_management_snapshot().await
    }

    async fn load_management_snapshot() -> Result<ManagementSnapshot, String> {
        let host = request_result::<HostSnapshot>("GET", "/api/1.0", None).await?;
        let images = request_result::<Vec<ImageArtifact>>("GET", "/api/1.0/images", None).await?;
        Ok(ManagementSnapshot { host, images })
    }

    async fn execute_intent(intent: Intent) -> Result<(), String> {
        let envelope = match intent {
            Intent::StartInstance {
                id,
                expected_generation,
            } => {
                request_json::<Instance, _>(
                    "POST",
                    &format!("/api/1.0/instances/{}/start", id.0),
                    &InstanceLifecycleMutation {
                        expected_generation,
                    },
                )
                .await?
            }
            Intent::StopInstance {
                id,
                expected_generation,
                force,
            } => {
                request_json::<Instance, _>(
                    "POST",
                    &format!("/api/1.0/instances/{}/stop", id.0),
                    &StopInstanceMutation {
                        expected_generation,
                        force,
                    },
                )
                .await?
            }
            Intent::DeleteInstance {
                id,
                expected_generation,
            } => {
                request_json::<Instance, _>(
                    "DELETE",
                    &format!("/api/1.0/instances/{}", id.0),
                    &InstanceLifecycleMutation {
                        expected_generation,
                    },
                )
                .await?
            }
            _ => return Err("This management action is not available yet.".into()),
        };

        match envelope {
            ApiResponse::Result { .. } => {}
            ApiResponse::Accepted { operation } => wait_for_operation(operation).await?,
            ApiResponse::Error { error } => return Err(error.message),
        }
        install_snapshot(load_management_snapshot().await?);
        Ok(())
    }

    async fn wait_for_operation(mut operation: Operation) -> Result<(), String> {
        for attempt in 0..120 {
            match operation.state {
                OperationState::Succeeded => return Ok(()),
                OperationState::Failed => {
                    return Err(operation.error.map_or_else(
                        || "The host operation failed.".into(),
                        |error| error.message,
                    ));
                }
                OperationState::Cancelled => return Err("The host operation was cancelled.".into()),
                OperationState::Indeterminate | OperationState::Unknown => {
                    return Err("The host could not determine the operation outcome.".into());
                }
                OperationState::Queued | OperationState::Running => {}
            }
            if attempt == 119 {
                break;
            }
            TimeoutFuture::new(500).await;
            operation = request_result(
                "GET",
                &format!("/api/1.0/operations/{}", operation.id.0),
                None,
            )
            .await?;
        }
        Err("The host operation is still running; refresh to check its status.".into())
    }

    fn consume_fragment_token() -> Result<String, String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let hash = window
            .location()
            .hash()
            .map_err(|_| "management credential is unavailable")?;
        let token = hash.strip_prefix("#token=").unwrap_or_default();
        if token.len() < 32 || token.len() > 512 || token.chars().any(char::is_whitespace) {
            return Err("Open this host with a valid management credential.".into());
        }
        window
            .history()
            .and_then(|history| history.replace_state_with_url(&JsValue::NULL, "", Some("/")))
            .map_err(|_| {
                "management credential could not be removed from the address bar".to_owned()
            })?;
        Ok(token.to_owned())
    }

    async fn request_result<T: DeserializeOwned>(
        method: &str,
        path: &str,
        body: Option<String>,
    ) -> Result<T, String> {
        match request_api(method, path, body).await? {
            ApiResponse::Result { data, .. } => Ok(data),
            ApiResponse::Error { error } => Err(error.message),
            ApiResponse::Accepted { .. } => {
                Err("management response status is inconsistent".into())
            }
        }
    }

    async fn request_json<T: DeserializeOwned, B: Serialize>(
        method: &str,
        path: &str,
        body: &B,
    ) -> Result<ApiResponse<T>, String> {
        let body =
            serde_json::to_string(body).map_err(|_| "management request could not be encoded")?;
        request_api(method, path, Some(body)).await
    }

    async fn request_api<T: DeserializeOwned>(
        method: &str,
        path: &str,
        body: Option<String>,
    ) -> Result<ApiResponse<T>, String> {
        let token = TOKEN
            .with(|slot| slot.get().cloned())
            .ok_or_else(|| "management credential is unavailable".to_owned())?;
        let headers = Headers::new().map_err(|_| "request headers are unavailable")?;
        headers
            .set("Authorization", &format!("Bearer {token}"))
            .map_err(|_| "management credential is invalid")?;
        headers
            .set("Accept", "application/json")
            .map_err(|_| "request headers are unavailable")?;
        if body.is_some() {
            headers
                .set("Content-Type", "application/json")
                .map_err(|_| "request headers are unavailable")?;
        }
        let options = RequestInit::new();
        options.set_method(method);
        options.set_headers(&headers);
        if let Some(body) = body {
            options.set_body(&JsValue::from_str(&body));
        }
        let request = Request::new_with_str_and_init(path, &options)
            .map_err(|_| "management request could not be created")?;
        let response = JsFuture::from(
            web_sys::window()
                .ok_or_else(|| "browser window is unavailable".to_owned())?
                .fetch_with_request(&request),
        )
        .await
        .map_err(|_| "management gateway is unreachable")?
        .dyn_into::<Response>()
        .map_err(|_| "management gateway returned an invalid response")?;
        let text = JsFuture::from(
            response
                .text()
                .map_err(|_| "management response is unreadable")?,
        )
        .await
        .map_err(|_| "management response is unreadable")?
        .as_string()
        .ok_or_else(|| "management response is not text".to_owned())?;
        let envelope = serde_json::from_str(&text)
            .map_err(|_| "management response does not match the API contract")?;
        if response.ok() {
            Ok(envelope)
        } else {
            match envelope {
                ApiResponse::Error { error } => Err(error.message),
                ApiResponse::Accepted { .. } | ApiResponse::Result { .. } => {
                    Err("management response status is inconsistent".into())
                }
            }
        }
    }

    fn install_snapshot(snapshot: ManagementSnapshot) {
        APPLICATION.with(|slot| {
            if let Some(application) = slot.get() {
                application.update(|cx| kernmux_web::install_management_snapshot(snapshot, cx));
            }
        });
    }

    fn fail_shell(message: String) {
        APPLICATION.with(|slot| {
            if let Some(application) = slot.get() {
                application.update(|cx| kernmux_web::fail_management_shell(message, cx));
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("the Kernmux web client must be built for wasm32-unknown-unknown");
}
