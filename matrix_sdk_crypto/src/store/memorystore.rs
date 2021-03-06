// Copyright 2020 The Matrix.org Foundation C.I.C.
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

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use dashmap::DashSet;
use matrix_sdk_common::{
    identifiers::{DeviceId, RoomId, UserId},
    locks::Mutex,
};

use super::{Account, CryptoStore, InboundGroupSession, Result, Session};
use crate::{
    device::ReadOnlyDevice,
    memory_stores::{DeviceStore, GroupSessionStore, ReadOnlyUserDevices, SessionStore},
};
#[derive(Debug, Clone)]
pub struct MemoryStore {
    sessions: SessionStore,
    inbound_group_sessions: GroupSessionStore,
    tracked_users: Arc<DashSet<UserId>>,
    users_for_key_query: Arc<DashSet<UserId>>,
    devices: DeviceStore,
}

impl MemoryStore {
    pub fn new() -> Self {
        MemoryStore {
            sessions: SessionStore::new(),
            inbound_group_sessions: GroupSessionStore::new(),
            tracked_users: Arc::new(DashSet::new()),
            users_for_key_query: Arc::new(DashSet::new()),
            devices: DeviceStore::new(),
        }
    }
}

#[async_trait]
impl CryptoStore for MemoryStore {
    async fn load_account(&self) -> Result<Option<Account>> {
        Ok(None)
    }

    async fn save_account(&self, _: Account) -> Result<()> {
        Ok(())
    }

    async fn save_sessions(&self, sessions: &[Session]) -> Result<()> {
        for session in sessions {
            let _ = self.sessions.add(session.clone()).await;
        }

        Ok(())
    }

    async fn get_sessions(&self, sender_key: &str) -> Result<Option<Arc<Mutex<Vec<Session>>>>> {
        Ok(self.sessions.get(sender_key))
    }

    async fn save_inbound_group_session(&self, session: InboundGroupSession) -> Result<bool> {
        Ok(self.inbound_group_sessions.add(session))
    }

    async fn get_inbound_group_session(
        &self,
        room_id: &RoomId,
        sender_key: &str,
        session_id: &str,
    ) -> Result<Option<InboundGroupSession>> {
        Ok(self
            .inbound_group_sessions
            .get(room_id, sender_key, session_id))
    }

    fn users_for_key_query(&self) -> HashSet<UserId> {
        #[allow(clippy::map_clone)]
        self.users_for_key_query.iter().map(|u| u.clone()).collect()
    }

    fn is_user_tracked(&self, user_id: &UserId) -> bool {
        self.tracked_users.contains(user_id)
    }

    fn has_users_for_key_query(&self) -> bool {
        !self.users_for_key_query.is_empty()
    }

    async fn update_tracked_user(&self, user: &UserId, dirty: bool) -> Result<bool> {
        if dirty {
            self.users_for_key_query.insert(user.clone());
        } else {
            self.users_for_key_query.remove(user);
        }

        Ok(self.tracked_users.insert(user.clone()))
    }

    async fn get_device(
        &self,
        user_id: &UserId,
        device_id: &DeviceId,
    ) -> Result<Option<ReadOnlyDevice>> {
        Ok(self.devices.get(user_id, device_id))
    }

    async fn delete_device(&self, device: ReadOnlyDevice) -> Result<()> {
        let _ = self.devices.remove(device.user_id(), device.device_id());
        Ok(())
    }

    async fn get_user_devices(&self, user_id: &UserId) -> Result<ReadOnlyUserDevices> {
        Ok(self.devices.user_devices(user_id))
    }

    async fn save_devices(&self, devices: &[ReadOnlyDevice]) -> Result<()> {
        for device in devices {
            let _ = self.devices.add(device.clone());
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::{
        device::test::get_device,
        olm::{test::get_account_and_session, InboundGroupSession},
        store::{memorystore::MemoryStore, CryptoStore},
    };
    use matrix_sdk_common::identifiers::room_id;

    #[tokio::test]
    async fn test_session_store() {
        let (account, session) = get_account_and_session().await;
        let store = MemoryStore::new();

        assert!(store.load_account().await.unwrap().is_none());
        store.save_account(account).await.unwrap();

        store.save_sessions(&[session.clone()]).await.unwrap();

        let sessions = store
            .get_sessions(&session.sender_key)
            .await
            .unwrap()
            .unwrap();
        let sessions = sessions.lock().await;

        let loaded_session = &sessions[0];

        assert_eq!(&session, loaded_session);
    }

    #[tokio::test]
    async fn test_group_session_store() {
        let (account, _) = get_account_and_session().await;
        let room_id = room_id!("!test:localhost");

        let (outbound, _) = account
            .create_group_session_pair(&room_id, Default::default())
            .await
            .unwrap();
        let inbound = InboundGroupSession::new(
            "test_key",
            "test_key",
            &room_id,
            outbound.session_key().await,
        )
        .unwrap();

        let store = MemoryStore::new();
        let _ = store
            .save_inbound_group_session(inbound.clone())
            .await
            .unwrap();

        let loaded_session = store
            .get_inbound_group_session(&room_id, "test_key", outbound.session_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(inbound, loaded_session);
    }

    #[tokio::test]
    async fn test_device_store() {
        let device = get_device();
        let store = MemoryStore::new();

        store.save_devices(&[device.clone()]).await.unwrap();

        let loaded_device = store
            .get_device(device.user_id(), device.device_id())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(device, loaded_device);

        let user_devices = store.get_user_devices(device.user_id()).await.unwrap();

        assert_eq!(user_devices.keys().next().unwrap(), device.device_id());
        assert_eq!(user_devices.devices().next().unwrap(), &device);

        let loaded_device = user_devices.get(device.device_id()).unwrap();

        assert_eq!(device, loaded_device);

        store.delete_device(device.clone()).await.unwrap();
        assert!(store
            .get_device(device.user_id(), device.device_id())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_tracked_users() {
        let device = get_device();
        let store = MemoryStore::new();

        assert!(store
            .update_tracked_user(device.user_id(), false)
            .await
            .unwrap());
        assert!(!store
            .update_tracked_user(device.user_id(), false)
            .await
            .unwrap());

        assert!(store.is_user_tracked(device.user_id()));
    }
}
