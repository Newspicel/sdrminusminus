use std::f64::consts::{PI, TAU};

use crate::{OrbitError, Tle, time::sidereal_angle};

mod deep;

use deep::DeepSpace;

pub const EARTH_RADIUS_KM: f64 = 6_378.135;
const MU: f64 = 398_600.8;
const J2: f64 = 0.001_082_616;
const J3: f64 = -0.000_002_538_81;
const J4: f64 = -0.000_001_655_97;
const J3OJ2: f64 = J3 / J2;
const TWO_THIRDS: f64 = 2.0 / 3.0;
const SMALL: f64 = 1.5e-12;
const DEEP_PERIOD_MIN: f64 = 225.0;

fn xke() -> f64 {
    60.0 / (EARTH_RADIUS_KM.powi(3) / MU).sqrt()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub position_km: [f64; 3],
    pub velocity_km_s: [f64; 3],
}

#[derive(Clone, Debug)]
pub struct Propagator {
    pub(crate) epoch_jd: f64,
    near: Near,
    deep: Option<Box<DeepSpace>>,
}

#[derive(Clone, Debug, Default)]
struct Near {
    bstar: f64,
    ecco: f64,
    inclo: f64,
    nodeo: f64,
    argpo: f64,
    mo: f64,
    no: f64,
    simple: bool,
    aycof: f64,
    con41: f64,
    cc1: f64,
    cc4: f64,
    cc5: f64,
    d2: f64,
    d3: f64,
    d4: f64,
    delmo: f64,
    eta: f64,
    argpdot: f64,
    omgcof: f64,
    sinmao: f64,
    t2cof: f64,
    t3cof: f64,
    t4cof: f64,
    t5cof: f64,
    x1mth2: f64,
    x7thm1: f64,
    mdot: f64,
    nodedot: f64,
    xlcof: f64,
    xmcof: f64,
    nodecf: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Mean {
    pub(crate) em: f64,
    pub(crate) argpm: f64,
    pub(crate) inclm: f64,
    pub(crate) mm: f64,
    pub(crate) nodem: f64,
    pub(crate) nm: f64,
}

impl Propagator {
    pub fn new(tle: &Tle) -> Result<Self, OrbitError> {
        let no_kozai = tle.mean_motion_rev_per_day * TAU / 1_440.0;
        if no_kozai <= 0.0 || !(0.0..1.0).contains(&tle.eccentricity) {
            return Err(OrbitError::Elements(
                "mean motion or eccentricity out of range",
            ));
        }
        let mut near = Near {
            bstar: tle.bstar,
            ecco: tle.eccentricity,
            inclo: tle.inclination_deg.to_radians(),
            nodeo: tle.raan_deg.to_radians(),
            argpo: tle.arg_perigee_deg.to_radians(),
            mo: tle.mean_anomaly_deg.to_radians(),
            ..Near::default()
        };
        let shape = Shape::recover(&mut near, no_kozai);
        let deep = near.derive(&shape, tle.epoch_jd);
        Ok(Self {
            epoch_jd: tle.epoch_jd,
            near,
            deep,
        })
    }

    #[must_use]
    pub fn minutes_since_epoch(&self, julian_date: f64) -> f64 {
        (julian_date - self.epoch_jd) * 1_440.0
    }

    pub fn propagate(&self, minutes: f64) -> Result<State, OrbitError> {
        let near = &self.near;
        let (mut mean, secular) = near.secular(minutes);
        if let Some(deep) = &self.deep {
            deep.secular(minutes, &mut mean);
        }
        if mean.nm <= 0.0 {
            return Err(OrbitError::Decayed("mean motion fell to zero"));
        }
        let am = (xke() / mean.nm).powf(TWO_THIRDS) * secular.tempa * secular.tempa;
        mean.nm = xke() / am.powf(1.5);
        mean.em -= secular.tempe;
        if mean.em >= 1.0 || mean.em < -0.001 {
            return Err(OrbitError::Decayed("eccentricity left the ellipse"));
        }
        mean.em = mean.em.max(1e-6);
        mean.mm += near.no * secular.templ;
        let xlm = mean.mm + mean.argpm + mean.nodem;
        mean.nodem %= TAU;
        mean.argpm %= TAU;
        mean.mm = (xlm % TAU - mean.argpm - mean.nodem) % TAU;
        let mut osc = Osculating {
            ep: mean.em,
            xincp: mean.inclm,
            argpp: mean.argpm,
            nodep: mean.nodem,
            mp: mean.mm,
        };
        let mut terms = Terms::from(near);
        if let Some(deep) = &self.deep {
            deep.periodics(minutes, &mut osc);
            if osc.xincp < 0.0 {
                osc.xincp = -osc.xincp;
                osc.nodep += PI;
                osc.argpp -= PI;
            }
            if !(0.0..=1.0).contains(&osc.ep) {
                return Err(OrbitError::Decayed(
                    "perturbed eccentricity left the ellipse",
                ));
            }
            terms = Terms::deep(osc.xincp);
        }
        short_period(&osc, &terms, am, mean.nm)
    }
}

pub(crate) struct Shape {
    pub(crate) cosio: f64,
    pub(crate) cosio2: f64,
    pub(crate) sinio: f64,
    pub(crate) eccsq: f64,
    pub(crate) omeosq: f64,
    pub(crate) rteosq: f64,
    ao: f64,
    posq: f64,
    rp: f64,
    con42: f64,
}

impl Shape {
    fn recover(near: &mut Near, no_kozai: f64) -> Self {
        let eccsq = near.ecco * near.ecco;
        let omeosq = 1.0 - eccsq;
        let rteosq = omeosq.sqrt();
        let cosio = near.inclo.cos();
        let cosio2 = cosio * cosio;
        let ak = (xke() / no_kozai).powf(TWO_THIRDS);
        let d1 = 0.75 * J2 * (3.0 * cosio2 - 1.0) / (rteosq * omeosq);
        let mut del = d1 / (ak * ak);
        let adel = ak * (1.0 - del * del - del * (1.0 / 3.0 + 134.0 * del * del / 81.0));
        del = d1 / (adel * adel);
        near.no = no_kozai / (1.0 + del);
        let ao = (xke() / near.no).powf(TWO_THIRDS);
        let po = ao * omeosq;
        let con42 = 1.0 - 5.0 * cosio2;
        near.con41 = -con42 - cosio2 - cosio2;
        Self {
            cosio,
            cosio2,
            sinio: near.inclo.sin(),
            eccsq,
            omeosq,
            rteosq,
            ao,
            posq: po * po,
            rp: ao * (1.0 - near.ecco),
            con42,
        }
    }
}

struct Secular {
    tempa: f64,
    tempe: f64,
    templ: f64,
}

impl Near {
    fn derive(&mut self, shape: &Shape, epoch_jd: f64) -> Option<Box<DeepSpace>> {
        let ss = 78.0 / EARTH_RADIUS_KM + 1.0;
        let qzms2t = ((120.0 - 78.0) / EARTH_RADIUS_KM).powi(4);
        self.simple = shape.rp < 220.0 / EARTH_RADIUS_KM + 1.0;
        let mut sfour = ss;
        let mut qzms24 = qzms2t;
        let perige = (shape.rp - 1.0) * EARTH_RADIUS_KM;
        if perige < 156.0 {
            sfour = if perige < 98.0 { 20.0 } else { perige - 78.0 };
            qzms24 = ((120.0 - sfour) / EARTH_RADIUS_KM).powi(4);
            sfour = sfour / EARTH_RADIUS_KM + 1.0;
        }
        let ao = shape.ao;
        let pinvsq = 1.0 / shape.posq;
        let tsi = 1.0 / (ao - sfour);
        self.eta = ao * self.ecco * tsi;
        let etasq = self.eta * self.eta;
        let eeta = self.ecco * self.eta;
        let psisq = (1.0 - etasq).abs();
        let coef = qzms24 * tsi.powi(4);
        let coef1 = coef / psisq.powf(3.5);
        let cc2 = coef1
            * self.no
            * (ao * (1.0 + 1.5 * etasq + eeta * (4.0 + etasq))
                + 0.375 * J2 * tsi / psisq * self.con41 * (8.0 + 3.0 * etasq * (8.0 + etasq)));
        self.cc1 = self.bstar * cc2;
        let cc3 = if self.ecco > 1.0e-4 {
            -2.0 * coef * tsi * J3OJ2 * self.no * shape.sinio / self.ecco
        } else {
            0.0
        };
        self.x1mth2 = 1.0 - shape.cosio2;
        self.cc4 = 2.0
            * self.no
            * coef1
            * ao
            * shape.omeosq
            * (self.eta * (2.0 + 0.5 * etasq) + self.ecco * (0.5 + 2.0 * etasq)
                - J2 * tsi / (ao * psisq)
                    * (-3.0 * self.con41 * (1.0 - 2.0 * eeta + etasq * (1.5 - 0.5 * eeta))
                        + 0.75
                            * self.x1mth2
                            * (2.0 * etasq - eeta * (1.0 + etasq))
                            * (2.0 * self.argpo).cos()));
        self.cc5 = 2.0 * coef1 * ao * shape.omeosq * (1.0 + 2.75 * (etasq + eeta) + eeta * etasq);
        self.rates(shape, pinvsq);
        self.omgcof = self.bstar * cc3 * self.argpo.cos();
        self.xmcof = if self.ecco > 1.0e-4 {
            -TWO_THIRDS * coef * self.bstar / eeta
        } else {
            0.0
        };
        self.nodecf = 3.5 * shape.omeosq * (-1.5 * J2 * pinvsq * self.no * shape.cosio) * self.cc1;
        self.t2cof = 1.5 * self.cc1;
        self.xlcof = long_period_coefficient(shape.sinio, shape.cosio);
        self.aycof = -0.5 * J3OJ2 * shape.sinio;
        self.delmo = (1.0 + self.eta * self.mo.cos()).powi(3);
        self.sinmao = self.mo.sin();
        self.x7thm1 = 7.0 * shape.cosio2 - 1.0;
        if TAU / self.no >= DEEP_PERIOD_MIN {
            self.simple = true;
            let gsto = sidereal_angle(epoch_jd);
            return Some(Box::new(DeepSpace::new(self, shape, epoch_jd, gsto)));
        }
        if !self.simple {
            self.drag_terms(ao, tsi, sfour);
        }
        None
    }

    fn rates(&mut self, shape: &Shape, pinvsq: f64) {
        let cosio2 = shape.cosio2;
        let cosio4 = cosio2 * cosio2;
        let temp1 = 1.5 * J2 * pinvsq * self.no;
        let temp2 = 0.5 * temp1 * J2 * pinvsq;
        let temp3 = -0.468_75 * J4 * pinvsq * pinvsq * self.no;
        self.mdot = self.no
            + 0.5 * temp1 * shape.rteosq * self.con41
            + 0.0625 * temp2 * shape.rteosq * (13.0 - 78.0 * cosio2 + 137.0 * cosio4);
        self.argpdot = -0.5 * temp1 * shape.con42
            + 0.0625 * temp2 * (7.0 - 114.0 * cosio2 + 395.0 * cosio4)
            + temp3 * (3.0 - 36.0 * cosio2 + 49.0 * cosio4);
        let xhdot1 = -temp1 * shape.cosio;
        self.nodedot = xhdot1
            + (0.5 * temp2 * (4.0 - 19.0 * cosio2) + 2.0 * temp3 * (3.0 - 7.0 * cosio2))
                * shape.cosio;
    }

    fn drag_terms(&mut self, ao: f64, tsi: f64, sfour: f64) {
        let cc1sq = self.cc1 * self.cc1;
        self.d2 = 4.0 * ao * tsi * cc1sq;
        let temp = self.d2 * tsi * self.cc1 / 3.0;
        self.d3 = (17.0 * ao + sfour) * temp;
        self.d4 = 0.5 * temp * ao * tsi * (221.0 * ao + 31.0 * sfour) * self.cc1;
        self.t3cof = self.d2 + 2.0 * cc1sq;
        self.t4cof = 0.25 * (3.0 * self.d3 + self.cc1 * (12.0 * self.d2 + 10.0 * cc1sq));
        self.t5cof = 0.2
            * (3.0 * self.d4
                + 12.0 * self.cc1 * self.d3
                + 6.0 * self.d2 * self.d2
                + 15.0 * cc1sq * (2.0 * self.d2 + cc1sq));
    }

    fn secular(&self, t: f64) -> (Mean, Secular) {
        let xmdf = self.mo + self.mdot * t;
        let argpdf = self.argpo + self.argpdot * t;
        let nodedf = self.nodeo + self.nodedot * t;
        let t2 = t * t;
        let mut mean = Mean {
            em: self.ecco,
            argpm: argpdf,
            inclm: self.inclo,
            mm: xmdf,
            nodem: nodedf + self.nodecf * t2,
            nm: self.no,
        };
        let mut secular = Secular {
            tempa: 1.0 - self.cc1 * t,
            tempe: self.bstar * self.cc4 * t,
            templ: self.t2cof * t2,
        };
        if !self.simple {
            let delomg = self.omgcof * t;
            let delm = self.xmcof * ((1.0 + self.eta * xmdf.cos()).powi(3) - self.delmo);
            let temp = delomg + delm;
            mean.mm = xmdf + temp;
            mean.argpm = argpdf - temp;
            let t3 = t2 * t;
            let t4 = t3 * t;
            secular.tempa -= self.d2 * t2 + self.d3 * t3 + self.d4 * t4;
            secular.tempe += self.bstar * self.cc5 * (mean.mm.sin() - self.sinmao);
            secular.templ += self.t3cof * t3 + t4 * (self.t4cof + t * self.t5cof);
        }
        (mean, secular)
    }
}

fn long_period_coefficient(sin_incl: f64, cos_incl: f64) -> f64 {
    let denominator = if (cos_incl + 1.0).abs() > SMALL {
        1.0 + cos_incl
    } else {
        SMALL
    };
    -0.25 * J3OJ2 * sin_incl * (3.0 + 5.0 * cos_incl) / denominator
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Osculating {
    pub(crate) ep: f64,
    pub(crate) xincp: f64,
    pub(crate) argpp: f64,
    pub(crate) nodep: f64,
    pub(crate) mp: f64,
}

struct Terms {
    aycof: f64,
    xlcof: f64,
    con41: f64,
    x1mth2: f64,
    x7thm1: f64,
}

impl From<&Near> for Terms {
    fn from(near: &Near) -> Self {
        Self {
            aycof: near.aycof,
            xlcof: near.xlcof,
            con41: near.con41,
            x1mth2: near.x1mth2,
            x7thm1: near.x7thm1,
        }
    }
}

impl Terms {
    fn deep(inclination: f64) -> Self {
        let (sinip, cosip) = inclination.sin_cos();
        let cosisq = cosip * cosip;
        Self {
            aycof: -0.5 * J3OJ2 * sinip,
            xlcof: long_period_coefficient(sinip, cosip),
            con41: 3.0 * cosisq - 1.0,
            x1mth2: 1.0 - cosisq,
            x7thm1: 7.0 * cosisq - 1.0,
        }
    }
}

fn short_period(osc: &Osculating, terms: &Terms, am: f64, nm: f64) -> Result<State, OrbitError> {
    let (sinip, cosip) = osc.xincp.sin_cos();
    let axnl = osc.ep * osc.argpp.cos();
    let temp = 1.0 / (am * (1.0 - osc.ep * osc.ep));
    let aynl = osc.ep * osc.argpp.sin() + temp * terms.aycof;
    let xl = osc.mp + osc.argpp + osc.nodep + temp * terms.xlcof * axnl;
    let u = (xl - osc.nodep) % TAU;
    let (sineo1, coseo1) = kepler(u, axnl, aynl);
    let ecose = axnl * coseo1 + aynl * sineo1;
    let esine = axnl * sineo1 - aynl * coseo1;
    let el2 = axnl * axnl + aynl * aynl;
    let pl = am * (1.0 - el2);
    if pl < 0.0 {
        return Err(OrbitError::Decayed("semi-latus rectum went negative"));
    }
    let rl = am * (1.0 - ecose);
    let rdotl = am.sqrt() * esine / rl;
    let rvdotl = pl.sqrt() / rl;
    let betal = (1.0 - el2).sqrt();
    let temp = esine / (1.0 + betal);
    let sinu = am / rl * (sineo1 - aynl - axnl * temp);
    let cosu = am / rl * (coseo1 - axnl + aynl * temp);
    let su = sinu.atan2(cosu);
    let sin2u = (cosu + cosu) * sinu;
    let cos2u = 1.0 - 2.0 * sinu * sinu;
    let temp = 1.0 / pl;
    let temp1 = 0.5 * J2 * temp;
    let temp2 = temp1 * temp;
    let mrt = rl * (1.0 - 1.5 * temp2 * betal * terms.con41) + 0.5 * temp1 * terms.x1mth2 * cos2u;
    if mrt < 1.0 {
        return Err(OrbitError::Decayed("the satellite is below the surface"));
    }
    let su = su - 0.25 * temp2 * terms.x7thm1 * sin2u;
    let xnode = osc.nodep + 1.5 * temp2 * cosip * sin2u;
    let xinc = osc.xincp + 1.5 * temp2 * cosip * sinip * cos2u;
    let mvt = rdotl - nm * temp1 * terms.x1mth2 * sin2u / xke();
    let rvdot = rvdotl + nm * temp1 * (terms.x1mth2 * cos2u + 1.5 * terms.con41) / xke();
    Ok(orient(su, xnode, xinc, mrt, mvt, rvdot))
}

fn kepler(u: f64, axnl: f64, aynl: f64) -> (f64, f64) {
    let mut eo1 = u;
    let mut step = f64::MAX;
    let mut sin_cos = eo1.sin_cos();
    for _ in 0..10 {
        if step.abs() < 1.0e-12 {
            break;
        }
        sin_cos = eo1.sin_cos();
        let (sineo1, coseo1) = sin_cos;
        step = (u - aynl * coseo1 + axnl * sineo1 - eo1) / (1.0 - coseo1 * axnl - sineo1 * aynl);
        step = step.clamp(-0.95, 0.95);
        eo1 += step;
    }
    sin_cos
}

fn orient(su: f64, xnode: f64, xinc: f64, mrt: f64, mvt: f64, rvdot: f64) -> State {
    let (sinsu, cossu) = su.sin_cos();
    let (snod, cnod) = xnode.sin_cos();
    let (sini, cosi) = xinc.sin_cos();
    let xmx = -snod * cosi;
    let xmy = cnod * cosi;
    let u = [
        xmx * sinsu + cnod * cossu,
        xmy * sinsu + snod * cossu,
        sini * sinsu,
    ];
    let v = [
        xmx * cossu - cnod * sinsu,
        xmy * cossu - snod * sinsu,
        sini * cossu,
    ];
    let velocity_scale = EARTH_RADIUS_KM * xke() / 60.0;
    State {
        position_km: u.map(|axis| mrt * axis * EARTH_RADIUS_KM),
        velocity_km_s: [0, 1, 2].map(|i| (mvt * u[i] + rvdot * v[i]) * velocity_scale),
    }
}

#[cfg(test)]
mod tests;
