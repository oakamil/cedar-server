// Copyright (c) 2026 Omair Kamil
// See LICENSE file in root directory for license terms.

use std::error::Error;

use bluer::{
    rfcomm::{Profile, Role, Stream},
    Session, Uuid,
};
use cedar_elements::{
    cedar::{cedar_server::Cedar, EmptyMessage},
    cedar_bt::{
        cedar_bt_request::RequestOneof, cedar_bt_response::ResponseOneof,
        CedarBtRequest, CedarBtResponse, RequestType, ResponseStatus,
    },
};
use futures::StreamExt;
use log::{debug, error, info, warn};
use prost::{bytes::BytesMut, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct BluetoothServer<T> {
    cedar: T,
}

impl<T: Cedar> BluetoothServer<T> {
    pub fn new(cedar: T) -> Self {
        Self { cedar }
    }

    pub async fn serve_requests(
        &mut self,
    ) -> Result<(), Box<dyn Error + 'static>> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;

        let profile = Profile {
            uuid: Uuid::parse_str("4e5d4c88-2965-423f-9111-28a506720760")
                .unwrap(),
            name: Some("Cedar Control".to_string()),
            role: Some(Role::Server),
            channel: Some(15),
            require_authentication: Some(false),
            require_authorization: Some(false),
            auto_connect: Some(false),
            ..Default::default()
        };

        let mut profile_handle = session.register_profile(profile).await?;
        info!(
            "Running control channel on Bluetooth: {}",
            adapter.address().await?
        );

        loop {
            let req = profile_handle.next().await;
            if req.is_none() {
                return Ok(());
            }
            match req.unwrap().accept() {
                Ok(stream) => {
                    self.handle_connection(stream).await;
                }
                Err(e) => {
                    warn!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    async fn handle_connection(&mut self, mut stream: Stream) {
        info!("Starting to read from cedar control connection (Bluetooth)");
        let mut buffer = BytesMut::new();

        loop {
            match stream.read_buf(&mut buffer).await {
                Ok(0) => {
                    info!("Client disconnected");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Error reading from stream: {}", e);
                    break;
                }
            }
            loop {
                // Looping here in case we get multiple requests together
                if let Some(resp) =
                    self.process_incoming_data(&mut buffer).await
                {
                    if let Err(e) = stream.write_all(&resp).await {
                        warn!("Failed to send data to client: {}", e);
                    }
                } else {
                    break;
                }
            }
        }
    }

    async fn process_incoming_data(
        &mut self,
        buffer: &mut BytesMut,
    ) -> Option<Vec<u8>> {
        match Self::get_next_request(buffer) {
            Ok(None) => None,
            Ok(Some(req)) => {
                let resp = self.process_parsed_request(req).await;
                debug!("Generated response: {:?}", resp);
                Some(resp.encode_length_delimited_to_vec())
            }
            Err(_) => {
                // Clear the buffer since we don't know when the next properly
                // formatted request starts
                buffer.clear();
                Some(
                    CedarBtResponse {
                        response_status: ResponseStatus::MalformedInput as i32,
                        response_oneof: None,
                    }
                    .encode_length_delimited_to_vec(),
                )
            }
        }
    }

    fn get_next_request(
        buffer: &mut BytesMut,
    ) -> Result<Option<CedarBtRequest>, ()> {
        // Try to decode the length without consuming data from the buffer
        let mut check_slice = &buffer[..];
        match prost::decode_length_delimiter(&mut check_slice) {
            Err(_) => {
                // Proto length varint is up to 10 bytes long
                if buffer.len() >= 10 {
                    error!(
                        "Failed to decode length header from buffer of size {}",
                        buffer.len()
                    );
                    Err(())
                } else {
                    Ok(None)
                }
            }
            Ok(len) => {
                let header_len = buffer.len() - check_slice.len();
                let total_len = header_len + len;
                if buffer.len() >= total_len {
                    // Split the delimited proto to a new buffer
                    let msg_buf = buffer.split_to(total_len).freeze();
                    match CedarBtRequest::decode_length_delimited(msg_buf) {
                        Ok(req) => {
                            debug!("Received CedarBtRequest: {:?}", req);
                            Ok(Some(req))
                        }
                        Err(e) => {
                            error!("Failed to decode request: {}", e);
                            Err(())
                        }
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn process_parsed_request(
        &self,
        request: CedarBtRequest,
    ) -> CedarBtResponse {
        let req_type = match RequestType::try_from(request.request_type) {
            Ok(t) => t,
            Err(_) => {
                return CedarBtResponse {
                    response_status: ResponseStatus::UnknownRequest as i32,
                    response_oneof: None,
                }
            }
        };

        match req_type {
            RequestType::GetServerLog => {
                if let Some(RequestOneof::ServerLogRequest(req)) =
                    request.request_oneof
                {
                    match self
                        .cedar
                        .get_server_log(tonic::Request::new(req))
                        .await
                    {
                        Ok(resp) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: Some(
                                ResponseOneof::ServerLogResult(
                                    resp.into_inner(),
                                ),
                            ),
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::UpdateFixedSettings => {
                if let Some(RequestOneof::FixedSettings(req)) =
                    request.request_oneof
                {
                    match self
                        .cedar
                        .update_fixed_settings(tonic::Request::new(req))
                        .await
                    {
                        Ok(resp) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: Some(ResponseOneof::FixedSettings(
                                resp.into_inner(),
                            )),
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::ClearObserverLocation => {
                match self
                    .cedar
                    .clear_observer_location(tonic::Request::new(
                        EmptyMessage {},
                    ))
                    .await
                {
                    Ok(_) => CedarBtResponse {
                        response_status: ResponseStatus::Success as i32,
                        response_oneof: None,
                    },
                    Err(e) => Self::rpc_failed_response(e),
                }
            }
            RequestType::UpdateOperationSettings => {
                if let Some(RequestOneof::OperationSettings(req)) =
                    request.request_oneof
                {
                    match self
                        .cedar
                        .update_operation_settings(tonic::Request::new(req))
                        .await
                    {
                        Ok(resp) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: Some(
                                ResponseOneof::OperationSettings(
                                    resp.into_inner(),
                                ),
                            ),
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::UpdatePreferences => {
                if let Some(RequestOneof::Preferences(req)) =
                    request.request_oneof
                {
                    match self
                        .cedar
                        .update_preferences(tonic::Request::new(req))
                        .await
                    {
                        Ok(resp) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: Some(ResponseOneof::Preferences(
                                resp.into_inner(),
                            )),
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::GetFrame => {
                if let Some(RequestOneof::FrameRequest(req)) =
                    request.request_oneof
                {
                    match self.cedar.get_frame(tonic::Request::new(req)).await {
                        Ok(resp) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: Some(ResponseOneof::FrameResult(
                                resp.into_inner(),
                            )),
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::InitiateAction => {
                if let Some(RequestOneof::ActionRequest(req)) =
                    request.request_oneof
                {
                    match self
                        .cedar
                        .initiate_action(tonic::Request::new(req))
                        .await
                    {
                        Ok(_) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: None,
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::QueryCatalogEntries => {
                if let Some(RequestOneof::QueryCatalogRequest(req)) =
                    request.request_oneof
                {
                    match self
                        .cedar
                        .query_catalog_entries(tonic::Request::new(req))
                        .await
                    {
                        Ok(resp) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: Some(
                                ResponseOneof::QueryCatalogResponse(
                                    resp.into_inner(),
                                ),
                            ),
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::GetCatalogEntry => {
                if let Some(RequestOneof::CatalogEntryKey(req)) =
                    request.request_oneof
                {
                    match self
                        .cedar
                        .get_catalog_entry(tonic::Request::new(req))
                        .await
                    {
                        Ok(resp) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: Some(ResponseOneof::CatalogEntry(
                                resp.into_inner(),
                            )),
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            RequestType::GetCatalogDescriptions => {
                match self
                    .cedar
                    .get_catalog_descriptions(tonic::Request::new(
                        EmptyMessage {},
                    ))
                    .await
                {
                    Ok(resp) => CedarBtResponse {
                        response_status: ResponseStatus::Success as i32,
                        response_oneof: Some(
                            ResponseOneof::CatalogDescriptionResponse(
                                resp.into_inner(),
                            ),
                        ),
                    },
                    Err(e) => Self::rpc_failed_response(e),
                }
            }
            RequestType::GetObjectTypes => {
                match self
                    .cedar
                    .get_object_types(tonic::Request::new(EmptyMessage {}))
                    .await
                {
                    Ok(resp) => CedarBtResponse {
                        response_status: ResponseStatus::Success as i32,
                        response_oneof: Some(
                            ResponseOneof::ObjectTypeResponse(
                                resp.into_inner(),
                            ),
                        ),
                    },
                    Err(e) => Self::rpc_failed_response(e),
                }
            }
            RequestType::GetConstellations => {
                match self
                    .cedar
                    .get_constellations(tonic::Request::new(EmptyMessage {}))
                    .await
                {
                    Ok(resp) => CedarBtResponse {
                        response_status: ResponseStatus::Success as i32,
                        response_oneof: Some(
                            ResponseOneof::ConstellationResponse(
                                resp.into_inner(),
                            ),
                        ),
                    },
                    Err(e) => Self::rpc_failed_response(e),
                }
            }
            RequestType::GetBluetoothName => {
                match self
                    .cedar
                    .get_bluetooth_name(tonic::Request::new(EmptyMessage {}))
                    .await
                {
                    Ok(resp) => CedarBtResponse {
                        response_status: ResponseStatus::Success as i32,
                        response_oneof: Some(
                            ResponseOneof::GetBluetoothNameResponse(
                                resp.into_inner(),
                            ),
                        ),
                    },
                    Err(e) => Self::rpc_failed_response(e),
                }
            }
            RequestType::StartBonding => {
                match self
                    .cedar
                    .start_bonding(tonic::Request::new(EmptyMessage {}))
                    .await
                {
                    Ok(resp) => CedarBtResponse {
                        response_status: ResponseStatus::Success as i32,
                        response_oneof: Some(
                            ResponseOneof::StartBondingResponse(
                                resp.into_inner(),
                            ),
                        ),
                    },
                    Err(e) => Self::rpc_failed_response(e),
                }
            }
            RequestType::GetBondedDevices => {
                match self
                    .cedar
                    .get_bonded_devices(tonic::Request::new(EmptyMessage {}))
                    .await
                {
                    Ok(resp) => CedarBtResponse {
                        response_status: ResponseStatus::Success as i32,
                        response_oneof: Some(
                            ResponseOneof::GetBondedDevicesResponse(
                                resp.into_inner(),
                            ),
                        ),
                    },
                    Err(e) => Self::rpc_failed_response(e),
                }
            }
            RequestType::RemoveBond => {
                if let Some(RequestOneof::RemoveBondRequest(req)) =
                    request.request_oneof
                {
                    match self.cedar.remove_bond(tonic::Request::new(req)).await
                    {
                        Ok(_) => CedarBtResponse {
                            response_status: ResponseStatus::Success as i32,
                            response_oneof: None,
                        },
                        Err(e) => Self::rpc_failed_response(e),
                    }
                } else {
                    Self::invalid_req_response()
                }
            }
            _ => CedarBtResponse {
                response_status: ResponseStatus::UnknownRequest as i32,
                response_oneof: None,
            },
        }
    }

    fn rpc_failed_response(e: tonic::Status) -> CedarBtResponse {
        warn!("RPC error: {:?}", e);
        CedarBtResponse {
            response_status: ResponseStatus::RpcFailed as i32,
            response_oneof: None,
        }
    }

    fn invalid_req_response() -> CedarBtResponse {
        CedarBtResponse {
            response_status: ResponseStatus::InvalidRequest as i32,
            response_oneof: None,
        }
    }
}
