use sdrmm_wire::BroadcastData;

use self::{
    entity::Entity,
    group::{DataGroup, Kind, SegmentNumber, segment_payload},
    header::Header,
};

mod entity;
mod group;
mod header;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub struct Mot {
    transfer: Option<Transfer>,
}

impl Mot {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<BroadcastData>, &'static str> {
        let group = DataGroup::parse(bytes)?;
        if group.kind == Kind::Unsupported {
            return Ok(None);
        }
        let segment = group
            .segment
            .ok_or("MOT data group without segment number")?;
        let transport_id = group
            .transport_id
            .ok_or("MOT data group without TransportId")?;
        let payload = segment_payload(group.data)?;
        let abandoned = self.retire_other(transport_id);
        let transfer = self
            .transfer
            .get_or_insert_with(|| Transfer::new(transport_id));
        match transfer.receive(group.kind, segment, payload)? {
            Some(object) => Ok(Some(object)),
            None if abandoned => Err("Incomplete MOT object replaced"),
            None => Ok(None),
        }
    }

    fn retire_other(&mut self, transport_id: u16) -> bool {
        match self.transfer.take() {
            Some(transfer) if transfer.transport_id == transport_id => {
                self.transfer = Some(transfer);
                false
            }
            Some(transfer) => !transfer.finished,
            None => false,
        }
    }
}

#[derive(Debug)]
struct Transfer {
    transport_id: u16,
    header: Entity,
    body: Entity,
    finished: bool,
}

impl Transfer {
    fn new(transport_id: u16) -> Self {
        Self {
            transport_id,
            header: Entity::default(),
            body: Entity::default(),
            finished: false,
        }
    }

    fn receive(
        &mut self,
        kind: Kind,
        segment: SegmentNumber,
        payload: &[u8],
    ) -> Result<Option<BroadcastData>, &'static str> {
        if self.entity(kind).store(segment, payload).is_err() {
            *self = Self::new(self.transport_id);
            self.entity(kind).store(segment, payload).ok();
            return Err("MOT segment differs from its earlier copy");
        }
        if self.finished {
            return Ok(None);
        }
        let (Some(header), Some(body)) = (self.header.assembled(), self.body.assembled()) else {
            return Ok(None);
        };
        self.finished = true;
        object(&header, body).map(Some)
    }

    fn entity(&mut self, kind: Kind) -> &mut Entity {
        match kind {
            Kind::Header => &mut self.header,
            Kind::Body | Kind::Unsupported => &mut self.body,
        }
    }
}

fn object(header: &[u8], body: Vec<u8>) -> Result<BroadcastData, &'static str> {
    let header = Header::parse(header)?;
    if header.body_size != body.len() {
        return Err("MOT body size mismatch");
    }
    if header.compressed {
        return Err("Compressed MOT object not supported");
    }
    if header.scrambled {
        return Err("Scrambled MOT object not supported");
    }
    let media_type = header.media_type().to_owned();
    let name = header
        .content_name
        .ok_or("MOT header without ContentName")?;
    Ok(BroadcastData {
        protocol: None,
        label: Vec::new(),
        service_id: None,
        name,
        media_type,
        bytes: body,
    })
}
