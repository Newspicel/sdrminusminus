use super::TunerKind;
use crate::dongle::{
    chip::Chip,
    error::{Error, Result},
    usb::Transport,
};

const R82_CHIP_ID: u8 = 0x69;
const RESET_PIN: u8 = 4;

struct Signature {
    name: &'static str,
    addr: u8,
    reg: u8,
    mask: u8,
    value: u8,
}

const BEFORE_RESET: [Signature; 2] = [
    Signature {
        name: "Elonics E4000",
        addr: 0xc8,
        reg: 0x02,
        mask: 0xff,
        value: 0x40,
    },
    Signature {
        name: "Fitipower FC0013",
        addr: 0xc6,
        reg: 0x00,
        mask: 0xff,
        value: 0xa3,
    },
];

const AFTER_RESET: [Signature; 2] = [
    Signature {
        name: "FCI FC2580",
        addr: 0xac,
        reg: 0x01,
        mask: 0x7f,
        value: 0x56,
    },
    Signature {
        name: "Fitipower FC0012",
        addr: 0xc6,
        reg: 0x00,
        mask: 0xff,
        value: 0xa1,
    },
];

pub(crate) fn identify<T: Transport>(chip: &Chip<T>) -> Result<TunerKind> {
    let found = [TunerKind::R820T, TunerKind::R828D]
        .into_iter()
        .find(|kind| chip.i2c_register(kind.address(), 0x00).ok() == Some(R82_CHIP_ID));
    if let Some(kind) = found {
        return Ok(kind);
    }
    if let Some(name) = recognise(chip, &BEFORE_RESET) {
        return Err(Error::ForeignTuner(name));
    }
    pulse_reset(chip);
    match recognise(chip, &AFTER_RESET) {
        Some(name) => Err(Error::ForeignTuner(name)),
        None => Err(Error::NoTuner),
    }
}

fn recognise<T: Transport>(chip: &Chip<T>, signatures: &[Signature]) -> Option<&'static str> {
    signatures
        .iter()
        .find(|sig| {
            chip.i2c_register(sig.addr, sig.reg)
                .is_ok_and(|byte| byte & sig.mask == sig.value)
        })
        .map(|sig| sig.name)
}

fn pulse_reset<T: Transport>(chip: &Chip<T>) {
    chip.make_output(RESET_PIN).ok();
    chip.drive_pin(RESET_PIN, true).ok();
    chip.drive_pin(RESET_PIN, false).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dongle::fake::Fake;

    fn probe(fake: &Fake) -> Result<TunerKind> {
        identify(&Chip::new(fake.clone()))
    }

    #[test]
    fn an_r820t_answers_at_0x34() {
        assert_eq!(
            probe(&Fake::answering_at(Some(0x34))).unwrap(),
            TunerKind::R820T
        );
    }

    #[test]
    fn an_r828d_answers_at_0x74() {
        let fake = Fake::answering_at(Some(0x74));
        assert_eq!(probe(&fake).unwrap(), TunerKind::R828D);
        assert_eq!(fake.i2c_writes(0x34), [vec![0x00]]);
    }

    #[test]
    fn an_e4000_is_named_without_a_reset() {
        let fake = Fake::answering_at(None);
        fake.answer_i2c(0xc8, 0x02, 0x40);
        assert!(matches!(
            probe(&fake),
            Err(Error::ForeignTuner("Elonics E4000"))
        ));
        assert!(fake.block_writes().is_empty(), "no reset pulse");
    }

    #[test]
    fn an_fc2580_is_named_after_the_reset() {
        let fake = Fake::answering_at(None);
        fake.answer_i2c(0xac, 0x01, 0xd6);
        assert!(matches!(
            probe(&fake),
            Err(Error::ForeignTuner("FCI FC2580"))
        ));
        let gpo: Vec<_> = fake
            .block_writes()
            .into_iter()
            .filter(|(addr, _)| *addr == 0x3001)
            .collect();
        assert_eq!(gpo, [(0x3001, vec![0x10]), (0x3001, vec![0x00])]);
    }

    #[test]
    fn nothing_answering_is_no_supported_tuner() {
        let error = probe(&Fake::answering_at(None)).unwrap_err();
        assert!(matches!(error, Error::NoTuner));
        assert!(error.to_string().contains("R820T at 0x34, R828D at 0x74"));
    }
}
