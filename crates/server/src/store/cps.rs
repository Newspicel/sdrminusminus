use rusqlite::{OptionalExtension, Row, params};
use sdrmm_wire::cps::{
    Codeplug, CodeplugCounts, CpsCodeplugDetail, CpsCodeplugInfo, CpsCodeplugRequest, CpsDevice,
    CpsDeviceRequest, CpsUser, CpsUserRequest, MAX_CPS_NAME_LEN, MAX_CPS_NOTE_LEN,
};

use super::{Store, StoreError, now_rfc3339};

fn check_name(name: &str) -> Result<(), StoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_CPS_NAME_LEN {
        return Err(StoreError::CpsField("name"));
    }
    Ok(())
}

fn check_note(note: Option<&str>) -> Result<(), StoreError> {
    if note.is_some_and(|note| note.chars().count() > MAX_CPS_NOTE_LEN) {
        return Err(StoreError::CpsField("note"));
    }
    Ok(())
}

fn user_from(row: &Row<'_>) -> Result<CpsUser, rusqlite::Error> {
    Ok(CpsUser {
        id: row.get(0)?,
        name: row.get(1)?,
        callsign: row.get(2)?,
        dmr_id: row
            .get::<_, Option<i64>>(3)?
            .and_then(|id| u32::try_from(id).ok()),
        note: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn device_from(row: &Row<'_>) -> Result<CpsDevice, rusqlite::Error> {
    Ok(CpsDevice {
        id: row.get(0)?,
        name: row.get(1)?,
        model_id: row.get(2)?,
        port: row.get(3)?,
        serial_number: row.get(4)?,
        firmware: row.get(5)?,
        owner_id: row.get(6)?,
        note: row.get(7)?,
        created_at: row.get(8)?,
    })
}

const USER_COLUMNS: &str = "id, name, callsign, dmr_id, note, created_at";
const DEVICE_COLUMNS: &str =
    "id, name, model_id, port, serial_number, firmware, owner_id, note, created_at";

impl Store {
    pub fn create_cps_user(&self, request: &CpsUserRequest) -> Result<i64, StoreError> {
        check_name(&request.name)?;
        check_note(request.note.as_deref())?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO cps_users (name, callsign, dmr_id, note, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                request.name.trim(),
                request.callsign,
                request.dmr_id.map(i64::from),
                request.note,
                now_rfc3339()
            ],
        )
        .map_err(taken_name)?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_cps_user(&self, id: i64, request: &CpsUserRequest) -> Result<(), StoreError> {
        check_name(&request.name)?;
        check_note(request.note.as_deref())?;
        let conn = self.lock();
        let changed = conn
            .execute(
                "UPDATE cps_users SET name = ?2, callsign = ?3, dmr_id = ?4, note = ?5 WHERE id = ?1",
                params![
                    id,
                    request.name.trim(),
                    request.callsign,
                    request.dmr_id.map(i64::from),
                    request.note
                ],
            )
            .map_err(taken_name)?;
        (changed > 0)
            .then_some(())
            .ok_or(StoreError::CpsUserNotFound(id))
    }

    pub fn delete_cps_user(&self, id: i64) -> Result<(), StoreError> {
        let conn = self.lock();
        let changed = conn.execute("DELETE FROM cps_users WHERE id = ?1", params![id])?;
        (changed > 0)
            .then_some(())
            .ok_or(StoreError::CpsUserNotFound(id))
    }

    pub fn list_cps_users(&self) -> Result<Vec<CpsUser>, StoreError> {
        let conn = self.lock();
        let mut statement = conn.prepare(&format!(
            "SELECT {USER_COLUMNS} FROM cps_users ORDER BY name"
        ))?;
        let users = statement
            .query_map([], user_from)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(users)
    }

    pub fn cps_user(&self, id: i64) -> Result<CpsUser, StoreError> {
        let conn = self.lock();
        conn.query_row(
            &format!("SELECT {USER_COLUMNS} FROM cps_users WHERE id = ?1"),
            params![id],
            user_from,
        )
        .optional()?
        .ok_or(StoreError::CpsUserNotFound(id))
    }

    pub fn create_cps_device(&self, request: &CpsDeviceRequest) -> Result<i64, StoreError> {
        check_name(&request.name)?;
        check_note(request.note.as_deref())?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO cps_devices \
             (name, model_id, port, serial_number, firmware, owner_id, note, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                request.name.trim(),
                request.model_id,
                request.port,
                request.serial_number,
                request.firmware,
                request.owner_id,
                request.note,
                now_rfc3339()
            ],
        )
        .map_err(taken_name)?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_cps_device(&self, id: i64, request: &CpsDeviceRequest) -> Result<(), StoreError> {
        check_name(&request.name)?;
        check_note(request.note.as_deref())?;
        let conn = self.lock();
        let changed = conn
            .execute(
                "UPDATE cps_devices SET name = ?2, model_id = ?3, port = ?4, serial_number = ?5, \
                 firmware = ?6, owner_id = ?7, note = ?8 WHERE id = ?1",
                params![
                    id,
                    request.name.trim(),
                    request.model_id,
                    request.port,
                    request.serial_number,
                    request.firmware,
                    request.owner_id,
                    request.note
                ],
            )
            .map_err(taken_name)?;
        (changed > 0)
            .then_some(())
            .ok_or(StoreError::CpsDeviceNotFound(id))
    }

    pub fn delete_cps_device(&self, id: i64) -> Result<(), StoreError> {
        let conn = self.lock();
        let changed = conn.execute("DELETE FROM cps_devices WHERE id = ?1", params![id])?;
        (changed > 0)
            .then_some(())
            .ok_or(StoreError::CpsDeviceNotFound(id))
    }

    pub fn list_cps_devices(&self) -> Result<Vec<CpsDevice>, StoreError> {
        let conn = self.lock();
        let mut statement = conn.prepare(&format!(
            "SELECT {DEVICE_COLUMNS} FROM cps_devices ORDER BY name"
        ))?;
        let devices = statement
            .query_map([], device_from)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(devices)
    }

    pub fn cps_device(&self, id: i64) -> Result<CpsDevice, StoreError> {
        let conn = self.lock();
        conn.query_row(
            &format!("SELECT {DEVICE_COLUMNS} FROM cps_devices WHERE id = ?1"),
            params![id],
            device_from,
        )
        .optional()?
        .ok_or(StoreError::CpsDeviceNotFound(id))
    }

    pub fn store_cps_codeplug(
        &self,
        request: &CpsCodeplugRequest,
        image: Option<&[u8]>,
    ) -> Result<i64, StoreError> {
        check_name(&request.name)?;
        let body = serde_json::to_string(&request.codeplug)?;
        let now = now_rfc3339();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO cps_codeplugs \
             (name, model_id, device_id, user_id, created_at, updated_at, codeplug, image) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)",
            params![
                request.name.trim(),
                request.model_id,
                request.device_id,
                request.user_id,
                now,
                body,
                image
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_cps_codeplug(
        &self,
        id: i64,
        request: &CpsCodeplugRequest,
    ) -> Result<(), StoreError> {
        check_name(&request.name)?;
        let body = serde_json::to_string(&request.codeplug)?;
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE cps_codeplugs SET name = ?2, model_id = ?3, device_id = ?4, user_id = ?5, \
             updated_at = ?6, codeplug = ?7 WHERE id = ?1",
            params![
                id,
                request.name.trim(),
                request.model_id,
                request.device_id,
                request.user_id,
                now_rfc3339(),
                body
            ],
        )?;
        (changed > 0)
            .then_some(())
            .ok_or(StoreError::CpsCodeplugNotFound(id))
    }

    pub fn delete_cps_codeplug(&self, id: i64) -> Result<(), StoreError> {
        let conn = self.lock();
        let changed = conn.execute("DELETE FROM cps_codeplugs WHERE id = ?1", params![id])?;
        (changed > 0)
            .then_some(())
            .ok_or(StoreError::CpsCodeplugNotFound(id))
    }

    pub fn list_cps_codeplugs(&self) -> Result<Vec<CpsCodeplugInfo>, StoreError> {
        let conn = self.lock();
        let mut statement = conn.prepare(
            "SELECT id, name, model_id, device_id, user_id, created_at, updated_at, codeplug              FROM cps_codeplugs ORDER BY updated_at DESC",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    CpsCodeplugInfo {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        model_id: row.get(2)?,
                        device_id: row.get(3)?,
                        user_id: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        counts: CodeplugCounts::default(),
                    },
                    row.get::<_, String>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(mut info, body)| {
                info.counts = serde_json::from_str::<Codeplug>(&body)
                    .map(|codeplug| codeplug.counts())
                    .unwrap_or_default();
                info
            })
            .collect())
    }

    pub fn cps_codeplug(&self, id: i64) -> Result<CpsCodeplugDetail, StoreError> {
        let conn = self.lock();
        let (mut info, body) = conn
            .query_row(
                "SELECT id, name, model_id, device_id, user_id, created_at, updated_at, codeplug                  FROM cps_codeplugs WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        CpsCodeplugInfo {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            model_id: row.get(2)?,
                            device_id: row.get(3)?,
                            user_id: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                            counts: CodeplugCounts::default(),
                        },
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StoreError::CpsCodeplugNotFound(id))?;
        let codeplug: Codeplug = serde_json::from_str(&body)?;
        info.counts = codeplug.counts();
        Ok(CpsCodeplugDetail { info, codeplug })
    }

    pub fn cps_codeplug_image(&self, id: i64) -> Result<Option<Vec<u8>>, StoreError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT image FROM cps_codeplugs WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StoreError::CpsCodeplugNotFound(id))
    }

    pub fn set_cps_codeplug_image(&self, id: i64, image: &[u8]) -> Result<(), StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE cps_codeplugs SET image = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, image, now_rfc3339()],
        )?;
        (changed > 0)
            .then_some(())
            .ok_or(StoreError::CpsCodeplugNotFound(id))
    }
}

fn taken_name(error: rusqlite::Error) -> StoreError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            StoreError::CpsNameTaken
        }
        _ => StoreError::Db(error),
    }
}
