// Copyright (c) 2026 Omair Kamil
// See LICENSE file in root directory for license terms.

use std::error::Error;

use bluer::{
    rfcomm::{Profile, Role, Stream},
    Session, Uuid,
};
use futures::StreamExt;
use log::{debug, error, info, warn};
use prost::{bytes::BytesMut, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use cedar_elements::{
  cedar_bt::{CedarBtRequest, CedarBtResponse, RequestType, ResponseStatus},
};

pub struct BluetoothServer {}

impl BluetoothServer {
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
                    self.process_incoming_data(&mut buffer).await {
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
                Some(CedarBtResponse {
                        response_status: ResponseStatus::MalformedInput as i32,
                        response_oneof: None,
                }.encode_length_delimited_to_vec())
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

    async fn process_parsed_request(&self, request: CedarBtRequest) -> CedarBtResponse {
        // Placeholder
        CedarBtResponse {
            response_status: ResponseStatus::Success as i32,
            response_oneof: None,
        }
    }
}
