// Copyright (c) 2016 Chef Software Inc. and/or applicable contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The pull thread.
//!
//! This module handles pulling all the pushed rumors from every member off a ZMQ socket.

use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use habitat_net::server::ZMQ_CONTEXT;
use protobuf;
use zmq;

use server::Server;
use message::swim::{Rumor, Rumor_Type};
use trace::TraceKind;

/// Takes a reference to the server itself
pub struct Pull<'a> {
    pub server: &'a Server,
}

impl<'a> Pull<'a> {
    /// Create a new Pull
    pub fn new(server: &'a Server) -> Pull {
        Pull { server: server }
    }

    /// Run this thread. Creates a socket, binds to the `gossip_addr`, then processes messages as
    /// they are recevied. Uses a ZMQ pull socket, so inbound messages are fair-queued.
    pub fn run(&mut self) {
        let mut socket = (**ZMQ_CONTEXT)
            .as_mut()
            .socket(zmq::PULL)
            .expect("Failure to create the ZMQ pull socket");
        socket.set_linger(0).expect("Failure to set the ZMQ Pull socket to not linger");
        socket.set_tcp_keepalive(0)
            .expect("Failure to set the ZMQ Pull socket to not use keepalive");
        socket.bind(&format!("tcp://{}", self.server.gossip_addr()))
            .expect("Failure to bind the ZMQ Pull socket to the port");
        'recv: loop {
            if self.server.pause.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            let msg = match socket.recv_msg(0) {
                Ok(msg) => msg,
                Err(e) => {
                    error!("Error receiving message: {:?}", e);
                    continue 'recv;
                }
            };
            let mut proto: Rumor = match protobuf::parse_from_bytes(&msg) {
                Ok(proto) => proto,
                Err(e) => {
                    error!("Error parsing protobuf: {:?}", e);
                    continue 'recv;
                }
            };
            if self.server.check_blacklist(proto.get_from_id()) {
                warn!("Not processing message from {} - it is blacklisted",
                      proto.get_from_id());
                continue 'recv;
            }
            trace_it!(GOSSIP: &self.server, TraceKind::RecvRumor, proto.get_from_id(), proto.get_from_address(), &proto);
            match proto.get_field_type() {
                Rumor_Type::Member => {
                    let member = proto.mut_member().take_member().into();
                    let health = proto.mut_member().get_health().into();
                    let members = vec![(member, health)];
                    self.server.insert_from_rumors(members);
                }
            }
        }
    }
}
