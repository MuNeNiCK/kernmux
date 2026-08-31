#![cfg_attr(target_arch = "wasm32", no_main)]

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::{cell::OnceCell, rc::Rc};

    use gloo_timers::future::TimeoutFuture;
    use kernmux_api::v1::{
        CreateInstanceMutation, HostSnapshot, ImageArtifact, ImportImageMutation, Instance,
        InstanceLifecycleMutation, LoadManagedImageMutation, Operation, OperationState,
        ResourcePool, ResourcePoolMutation, Response as ApiResponse, StopInstanceMutation,
    };
    use kernmux_ui_model::{Intent, ManagementSnapshot};
    use kernmux_web::{ManagementBackend, ManagementFuture};
    use serde::{Serialize, de::DeserializeOwned};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Event, Headers, MediaQueryList, Request, RequestInit, Response};

    type ReducedMotionListener = (MediaQueryList, Closure<dyn FnMut(Event)>);

    thread_local! {
        static APPLICATION: OnceCell<gpui::ApplicationHandle> = const { OnceCell::new() };
        static REDUCED_MOTION: OnceCell<ReducedMotionListener> = const { OnceCell::new() };
    }

    #[derive(Clone)]
    struct BrowserBackend {
        credential: Result<String, String>,
    }

    impl ManagementBackend for BrowserBackend {
        fn load_snapshot(&self) -> ManagementFuture {
            let credential = self.credential.clone();
            Box::pin(async move {
                let token = credential?;
                load_management_snapshot(&token).await
            })
        }

        fn execute(&self, intent: Intent) -> ManagementFuture {
            let credential = self.credential.clone();
            Box::pin(async move {
                let token = credential?;
                execute_intent(intent, &token).await?;
                load_management_snapshot(&token).await
            })
        }
    }

    #[wasm_bindgen::prelude::wasm_bindgen(start)]
    pub fn start() {
        gpui_platform::web_init();
        let backend: Rc<dyn ManagementBackend> = Rc::new(BrowserBackend {
            credential: consume_fragment_token(),
        });
        let application = gpui_platform::application()
            .run_embedded(move |cx| kernmux_web::open_management_shell(cx, backend));
        APPLICATION.with(|slot| {
            assert!(slot.set(application).is_ok(), "application already started");
        });
        start_reduced_motion_preference();
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

    async fn load_management_snapshot(token: &str) -> Result<ManagementSnapshot, String> {
        let host = request_result::<HostSnapshot>("GET", "/api/1.0", None, token).await?;
        let images =
            request_result::<Vec<ImageArtifact>>("GET", "/api/1.0/images", None, token).await?;
        Ok(ManagementSnapshot { host, images })
    }

    async fn execute_intent(intent: Intent, token: &str) -> Result<(), String> {
        match intent {
            Intent::ConfigurePool {
                expected_generation,
                cpu_hardware_ids,
                memory_bytes,
            } => {
                finish(
                    request_json::<ResourcePool, _>(
                        "PUT",
                        "/api/1.0/resource-pool",
                        &ResourcePoolMutation {
                            expected_generation,
                            cpu_hardware_ids,
                            memory_bytes,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::CreateInstance {
                expected_generation,
                id,
                name,
                cpu_hardware_ids,
                memory_bytes,
            } => {
                finish(
                    request_json::<Instance, _>(
                        "POST",
                        "/api/1.0/instances",
                        &CreateInstanceMutation {
                            expected_generation,
                            id,
                            name,
                            cpu_hardware_ids,
                            memory_bytes,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::ImportImage {
                expected_generation,
                kind,
                source_path,
                expected_id,
            } => {
                finish(
                    request_json::<ImageArtifact, _>(
                        "POST",
                        "/api/1.0/images",
                        &ImportImageMutation {
                            expected_generation,
                            kind,
                            source_path,
                            expected_id,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::LoadInstanceImage {
                id,
                expected_generation,
                kernel_id,
                initrd_id,
                command_line,
            } => {
                finish(
                    request_json::<Instance, _>(
                        "POST",
                        &format!("/api/1.0/instances/{}/load-image", id.0),
                        &LoadManagedImageMutation {
                            expected_generation,
                            kernel_id,
                            initrd_id,
                            command_line,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::UnloadInstance {
                id,
                expected_generation,
            } => {
                finish(
                    request_json::<Instance, _>(
                        "POST",
                        &format!("/api/1.0/instances/{}/unload", id.0),
                        &InstanceLifecycleMutation {
                            expected_generation,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::StartInstance {
                id,
                expected_generation,
            } => {
                finish(
                    request_json::<Instance, _>(
                        "POST",
                        &format!("/api/1.0/instances/{}/start", id.0),
                        &InstanceLifecycleMutation {
                            expected_generation,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::StopInstance {
                id,
                expected_generation,
                force,
            } => {
                finish(
                    request_json::<Instance, _>(
                        "POST",
                        &format!("/api/1.0/instances/{}/stop", id.0),
                        &StopInstanceMutation {
                            expected_generation,
                            force,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::DeleteInstance {
                id,
                expected_generation,
            } => {
                finish(
                    request_json::<Instance, _>(
                        "DELETE",
                        &format!("/api/1.0/instances/{}", id.0),
                        &InstanceLifecycleMutation {
                            expected_generation,
                        },
                        token,
                    )
                    .await?,
                    token,
                )
                .await
            }
            Intent::Refresh => return Ok(()),
            _ => Err("This management action is not available yet.".into()),
        }
    }

    async fn finish<T>(envelope: ApiResponse<T>, token: &str) -> Result<(), String> {
        match envelope {
            ApiResponse::Result { .. } => Ok(()),
            ApiResponse::Accepted { operation } => wait_for_operation(operation, token).await,
            ApiResponse::Error { error } => Err(error.message),
        }
    }

    async fn wait_for_operation(mut operation: Operation, token: &str) -> Result<(), String> {
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
                token,
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
        token: &str,
    ) -> Result<T, String> {
        match request_api(method, path, body, token).await? {
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
        token: &str,
    ) -> Result<ApiResponse<T>, String> {
        let body =
            serde_json::to_string(body).map_err(|_| "management request could not be encoded")?;
        request_api(method, path, Some(body), token).await
    }

    async fn request_api<T: DeserializeOwned>(
        method: &str,
        path: &str,
        body: Option<String>,
        token: &str,
    ) -> Result<ApiResponse<T>, String> {
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
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("the Kernmux web client must be built for wasm32-unknown-unknown");
}
