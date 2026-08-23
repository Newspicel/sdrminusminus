use std::{collections::BTreeMap, sync::OnceLock};

use sdrmm_wire::cps::RadioModelDescriptor;

use crate::{RadioModel, anytone::D890Uv, radtel::Rt4D};

pub struct ModelRegistry {
    models: BTreeMap<String, Box<dyn RadioModel>>,
}

impl ModelRegistry {
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut registry = Self {
            models: BTreeMap::new(),
        };
        registry.register(Box::new(D890Uv));
        registry.register(Box::new(Rt4D));
        registry
    }

    pub fn register(&mut self, model: Box<dyn RadioModel>) {
        self.models.insert(model.descriptor().id, model);
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&dyn RadioModel> {
        self.models.get(id).map(AsRef::as_ref)
    }

    #[must_use]
    pub fn descriptors(&self) -> Vec<RadioModelDescriptor> {
        let mut descriptors: Vec<RadioModelDescriptor> = self
            .models
            .values()
            .map(|model| model.descriptor())
            .collect();
        descriptors.sort_by(|left, right| {
            left.manufacturer
                .cmp(&right.manufacturer)
                .then_with(|| left.model.cmp(&right.model))
        });
        descriptors
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn RadioModel> {
        self.models.values().map(AsRef::as_ref)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

#[must_use]
pub fn models() -> &'static ModelRegistry {
    static REGISTRY: OnceLock<ModelRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ModelRegistry::with_builtins)
}

#[must_use]
pub fn model(id: &str) -> Option<&'static dyn RadioModel> {
    models().get(id)
}

#[cfg(test)]
pub mod test_support {
    use sdrmm_wire::cps::{ChannelKind, Power, RadioFeatures, RadioLimits, RadioModelDescriptor};

    #[must_use]
    pub fn demo_descriptor() -> RadioModelDescriptor {
        RadioModelDescriptor {
            id: "demo".to_owned(),
            manufacturer: "Demo".to_owned(),
            model: "Demo".to_owned(),
            family: "demo".to_owned(),
            usb: Vec::new(),
            needs_explicit_selection: true,
            transfer_bytes: 0,
            limits: RadioLimits {
                channels: 1,
                contacts: 1,
                group_lists: 1,
                group_list_members: 1,
                zones: 1,
                zone_channels: 1,
                scan_lists: 1,
                scan_list_members: 1,
                radio_ids: 1,
                channel_name_len: 8,
                contact_name_len: 8,
                group_list_name_len: 8,
                zone_name_len: 8,
                scan_list_name_len: 8,
                radio_id_name_len: 8,
                rx_ranges: Vec::new(),
                tx_ranges: Vec::new(),
                powers: vec![Power::Low],
                modes: vec![ChannelKind::Fm],
                frequency_step_hz: 1,
                features: RadioFeatures::default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_model_is_listed_once_and_reachable_by_id() {
        let registry = models();
        assert_eq!(registry.len(), registry.descriptors().len());
        assert!(registry.get(crate::anytone::d890uv::MODEL_ID).is_some());
        assert!(registry.get(crate::radtel::rt4d::MODEL_ID).is_some());
        assert!(registry.get("nothing").is_none());
    }

    #[test]
    fn every_model_declares_regions_that_do_not_overlap() {
        for model in models().iter() {
            let mut regions = model.regions().to_vec();
            regions.sort_by_key(|region| region.addr);
            for pair in regions.windows(2) {
                assert!(
                    pair[0].end() <= pair[1].addr,
                    "{} overlaps {} in {}",
                    pair[0].name,
                    pair[1].name,
                    model.descriptor().id
                );
            }
        }
    }
}
