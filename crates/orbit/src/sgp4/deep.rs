use std::f64::consts::{PI, TAU};

use super::{Mean, Near, Osculating, Shape, TWO_THIRDS, xke};

const ZES: f64 = 0.016_75;
const ZEL: f64 = 0.054_90;
const ZNS: f64 = 1.194_59e-5;
const ZNL: f64 = 1.583_521_8e-4;
const C1SS: f64 = 2.986_479_7e-6;
const C1L: f64 = 4.796_806_5e-7;
const ZSINIS: f64 = 0.397_854_16;
const ZCOSIS: f64 = 0.917_448_67;
const ZCOSGS: f64 = 0.194_590_5;
const ZSINGS: f64 = -0.980_884_58;
const RPTIM: f64 = 4.375_269_088_011_3e-3;
const EPOCH_1950_JD: f64 = 2_433_281.5;
const STEP: f64 = 720.0;
const STEP2: f64 = 259_200.0;
const LOW_INCLINATION: f64 = 5.235_987_7e-2;

#[derive(Clone, Copy, Debug, Default)]
struct Body {
    s: [f64; 7],
    z1: f64,
    z2: f64,
    z3: f64,
    z11: f64,
    z12: f64,
    z13: f64,
    z21: f64,
    z22: f64,
    z23: f64,
    z31: f64,
    z32: f64,
    z33: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Periodic {
    e2: f64,
    e3: f64,
    i2: f64,
    i3: f64,
    l2: f64,
    l3: f64,
    l4: f64,
    gh2: f64,
    gh3: f64,
    gh4: f64,
    h2: f64,
    h3: f64,
}

#[derive(Clone, Copy, Debug, Default)]
enum Resonance {
    #[default]
    None,
    Synchronous {
        del: [f64; 3],
    },
    HalfDay {
        d: [f64; 10],
    },
}

#[derive(Clone, Debug)]
pub(super) struct DeepSpace {
    solar: Periodic,
    lunar: Periodic,
    zmos: f64,
    zmol: f64,
    dedt: f64,
    didt: f64,
    dmdt: f64,
    dnodt: f64,
    domdt: f64,
    resonance: Resonance,
    xfact: f64,
    xlamo: f64,
    gsto: f64,
    argpo: f64,
    argpdot: f64,
    no: f64,
}

struct Common {
    solar: Body,
    lunar: Body,
    solar_terms: Periodic,
    lunar_terms: Periodic,
    zmos: f64,
    zmol: f64,
    emsq: f64,
}

impl DeepSpace {
    pub(super) fn new(near: &Near, shape: &Shape, epoch_jd: f64, gsto: f64) -> Self {
        let common = common(near, shape, epoch_jd - EPOCH_1950_JD);
        let mut deep = Self {
            solar: common.solar_terms,
            lunar: common.lunar_terms,
            zmos: common.zmos,
            zmol: common.zmol,
            dedt: 0.0,
            didt: 0.0,
            dmdt: 0.0,
            dnodt: 0.0,
            domdt: 0.0,
            resonance: Resonance::None,
            xfact: 0.0,
            xlamo: 0.0,
            gsto,
            argpo: near.argpo,
            argpdot: near.argpdot,
            no: near.no,
        };
        deep.rates(near, shape, &common);
        deep.resonance(near, shape, common.emsq);
        deep
    }

    fn rates(&mut self, near: &Near, shape: &Shape, common: &Common) {
        let (sinim, cosim) = (shape.sinio, shape.cosio);
        let emsq = common.emsq;
        let s = &common.solar;
        let l = &common.lunar;
        let ses = s.s[0] * ZNS * s.s[4];
        let sis = s.s[1] * ZNS * (s.z11 + s.z13);
        let sls = -ZNS * s.s[2] * (s.z1 + s.z3 - 14.0 - 6.0 * emsq);
        let sghs = s.s[3] * ZNS * (s.z31 + s.z33 - 6.0);
        let equatorial = near.inclo < LOW_INCLINATION || near.inclo > PI - LOW_INCLINATION;
        let mut shs = if equatorial {
            0.0
        } else {
            -ZNS * s.s[1] * (s.z21 + s.z23)
        };
        if sinim != 0.0 {
            shs /= sinim;
        }
        let sgs = sghs - cosim * shs;
        self.dedt = ses + l.s[0] * ZNL * l.s[4];
        self.didt = sis + l.s[1] * ZNL * (l.z11 + l.z13);
        self.dmdt = sls - ZNL * l.s[2] * (l.z1 + l.z3 - 14.0 - 6.0 * emsq);
        let sghl = l.s[3] * ZNL * (l.z31 + l.z33 - 6.0);
        let shll = if equatorial {
            0.0
        } else {
            -ZNL * l.s[1] * (l.z21 + l.z23)
        };
        self.domdt = sgs + sghl;
        self.dnodt = shs;
        if sinim != 0.0 {
            self.domdt -= cosim / sinim * shll;
            self.dnodt += shll / sinim;
        }
    }

    fn resonance(&mut self, near: &Near, shape: &Shape, emsq: f64) {
        let nm = near.no;
        let em = near.ecco;
        let theta = self.gsto % TAU;
        let aonv = (nm / xke()).powf(TWO_THIRDS);
        if nm > 0.003_490_658_5 && nm < 0.005_235_987_7 {
            let (sinim, cosim) = (shape.sinio, shape.cosio);
            let g200 = 1.0 + emsq * (-2.5 + 0.8125 * emsq);
            let g310 = 1.0 + 2.0 * emsq;
            let g300 = 1.0 + emsq * (-6.0 + 6.609_37 * emsq);
            let f220 = 0.75 * (1.0 + cosim) * (1.0 + cosim);
            let f311 = 0.9375 * sinim * sinim * (1.0 + 3.0 * cosim) - 0.75 * (1.0 + cosim);
            let f330 = 1.875 * (1.0 + cosim).powi(3);
            let del1 = 3.0 * nm * nm * aonv * aonv;
            let del2 = 2.0 * del1 * f220 * g200 * 1.789_167_9e-6;
            let del3 = 3.0 * del1 * f330 * g300 * 2.212_301_5e-7 * aonv;
            let del1 = del1 * f311 * g310 * 2.146_074_8e-6 * aonv;
            self.resonance = Resonance::Synchronous {
                del: [del1, del2, del3],
            };
            self.xlamo = (near.mo + near.nodeo + near.argpo - theta) % TAU;
            self.xfact = near.mdot + near.argpdot + near.nodedot - RPTIM
                + self.dmdt
                + self.domdt
                + self.dnodt
                - near.no;
        } else if (8.26e-3..=9.24e-3).contains(&nm) && em >= 0.5 {
            self.resonance = Resonance::HalfDay {
                d: half_day_terms(shape, em, shape.eccsq, nm, aonv),
            };
            self.xlamo = (near.mo + near.nodeo + near.nodeo - theta - theta) % TAU;
            self.xfact =
                near.mdot + self.dmdt + 2.0 * (near.nodedot + self.dnodt - RPTIM) - near.no;
        }
    }

    pub(super) fn secular(&self, t: f64, mean: &mut Mean) {
        mean.em += self.dedt * t;
        mean.inclm += self.didt * t;
        mean.argpm += self.domdt * t;
        mean.nodem += self.dnodt * t;
        mean.mm += self.dmdt * t;
        if matches!(self.resonance, Resonance::None) {
            return;
        }
        let theta = (self.gsto + t * RPTIM) % TAU;
        let (xl, nm) = self.integrate(t);
        mean.mm = match self.resonance {
            Resonance::Synchronous { .. } => xl - mean.nodem - mean.argpm + theta,
            _ => xl - 2.0 * mean.nodem + 2.0 * theta,
        };
        mean.nm = nm;
    }

    fn integrate(&self, t: f64) -> (f64, f64) {
        let delt = if t > 0.0 { STEP } else { -STEP };
        let mut atime = 0.0;
        let mut xli = self.xlamo;
        let mut xni = self.no;
        loop {
            let (xndt, xnddt) = self.derivatives(atime, xli, xni);
            let xldot = xni + self.xfact;
            if (t - atime).abs() < STEP {
                let ft = t - atime;
                let nm = xni + xndt * ft + xnddt * ft * ft * 0.5;
                let xl = xli + xldot * ft + xndt * ft * ft * 0.5;
                return (xl, nm);
            }
            xli += xldot * delt + xndt * STEP2;
            xni += xndt * delt + xnddt * STEP2;
            atime += delt;
        }
    }

    fn derivatives(&self, atime: f64, xli: f64, xni: f64) -> (f64, f64) {
        let xldot = xni + self.xfact;
        match self.resonance {
            Resonance::Synchronous { del } => {
                let phases = [
                    xli - 0.131_309_08,
                    2.0 * (xli - 2.884_319_8),
                    3.0 * (xli - 0.374_480_87),
                ];
                let xndt: f64 = (0..3).map(|i| del[i] * phases[i].sin()).sum();
                let xnddt: f64 = (0..3)
                    .map(|i| (i + 1) as f64 * del[i] * phases[i].cos())
                    .sum();
                (xndt, xnddt * xldot)
            }
            Resonance::HalfDay { d } => {
                let xomi = self.argpo + self.argpdot * atime;
                let x2omi = xomi + xomi;
                let x2li = xli + xli;
                let angles = [
                    (x2omi + xli - 5.768_639_6, 1.0),
                    (xli - 5.768_639_6, 1.0),
                    (xomi + xli - 0.952_408_98, 1.0),
                    (-xomi + xli - 0.952_408_98, 1.0),
                    (x2omi + x2li - 1.801_499_8, 2.0),
                    (x2li - 1.801_499_8, 2.0),
                    (xomi + xli - 1.050_833_0, 1.0),
                    (-xomi + xli - 1.050_833_0, 1.0),
                    (xomi + x2li - 4.410_889_8, 2.0),
                    (-xomi + x2li - 4.410_889_8, 2.0),
                ];
                let xndt: f64 = angles
                    .iter()
                    .zip(d)
                    .map(|(&(angle, _), coefficient)| coefficient * angle.sin())
                    .sum();
                let xnddt: f64 = angles
                    .iter()
                    .zip(d)
                    .map(|(&(angle, order), coefficient)| order * coefficient * angle.cos())
                    .sum();
                (xndt, xnddt * xldot)
            }
            Resonance::None => (0.0, 0.0),
        }
    }

    pub(super) fn periodics(&self, t: f64, osc: &mut Osculating) {
        let solar = lunisolar(&self.solar, self.zmos + ZNS * t, ZES);
        let lunar = lunisolar(&self.lunar, self.zmol + ZNL * t, ZEL);
        let pe = solar[0] + lunar[0];
        let pinc = solar[1] + lunar[1];
        let pl = solar[2] + lunar[2];
        let mut pgh = solar[3] + lunar[3];
        let mut ph = solar[4] + lunar[4];
        osc.xincp += pinc;
        osc.ep += pe;
        let (sinip, cosip) = osc.xincp.sin_cos();
        if osc.xincp >= 0.2 {
            ph /= sinip;
            pgh -= cosip * ph;
            osc.argpp += pgh;
            osc.nodep += ph;
            osc.mp += pl;
            return;
        }
        let (sinop, cosop) = osc.nodep.sin_cos();
        let alfdp = sinip * sinop + ph * cosop + pinc * cosip * sinop;
        let betdp = sinip * cosop - ph * sinop + pinc * cosip * cosop;
        osc.nodep %= TAU;
        let xls = osc.mp + osc.argpp + cosip * osc.nodep + pl + pgh - pinc * osc.nodep * sinip;
        let xnoh = osc.nodep;
        osc.nodep = alfdp.atan2(betdp);
        if (xnoh - osc.nodep).abs() > PI {
            if osc.nodep < xnoh {
                osc.nodep += TAU;
            } else {
                osc.nodep -= TAU;
            }
        }
        osc.mp += pl;
        osc.argpp = xls - osc.mp - cosip * osc.nodep;
    }
}

fn lunisolar(terms: &Periodic, zm: f64, eccentricity: f64) -> [f64; 5] {
    let zf = zm + 2.0 * eccentricity * zm.sin();
    let sinzf = zf.sin();
    let f2 = 0.5 * sinzf * sinzf - 0.25;
    let f3 = -0.5 * sinzf * zf.cos();
    [
        terms.e2 * f2 + terms.e3 * f3,
        terms.i2 * f2 + terms.i3 * f3,
        terms.l2 * f2 + terms.l3 * f3 + terms.l4 * sinzf,
        terms.gh2 * f2 + terms.gh3 * f3 + terms.gh4 * sinzf,
        terms.h2 * f2 + terms.h3 * f3,
    ]
}

fn common(near: &Near, shape: &Shape, epoch: f64) -> Common {
    let (snodm, cnodm) = near.nodeo.sin_cos();
    let (sinomm, cosomm) = near.argpo.sin_cos();
    let emsq = near.ecco * near.ecco;
    let day = epoch + 18_261.5;
    let xnodce = (4.523_602_0 - 9.242_202_9e-4 * day) % TAU;
    let (stem, ctem) = xnodce.sin_cos();
    let zcosil = 0.913_751_64 - 0.035_680_96 * ctem;
    let zsinil = (1.0 - zcosil * zcosil).sqrt();
    let zsinhl = 0.089_683_511 * stem / zsinil;
    let zcoshl = (1.0 - zsinhl * zsinhl).sqrt();
    let gam = 5.835_151_4 + 0.001_944_368_0 * day;
    let zx = (0.397_854_16 * stem / zsinil).atan2(zcoshl * ctem + 0.917_448_67 * zsinhl * stem);
    let zx = gam + zx - xnodce;
    let frame = Frame {
        sinim: shape.sinio,
        cosim: shape.cosio,
        sinomm,
        cosomm,
        em: near.ecco,
        emsq,
        rtemsq: (1.0 - emsq).sqrt(),
        xnoi: 1.0 / near.no,
    };
    let solar = frame.body(ZCOSGS, ZSINGS, ZCOSIS, ZSINIS, cnodm, snodm, C1SS);
    let lunar = frame.body(
        zx.cos(),
        zx.sin(),
        zcosil,
        zsinil,
        zcoshl * cnodm + zsinhl * snodm,
        snodm * zcoshl - cnodm * zsinhl,
        C1L,
    );
    Common {
        solar_terms: periodic(&solar, emsq, ZES),
        lunar_terms: periodic(&lunar, emsq, ZEL),
        solar,
        lunar,
        zmol: (4.719_967_2 + 0.229_971_50 * day - gam) % TAU,
        zmos: (6.256_583_7 + 0.017_201_977 * day) % TAU,
        emsq,
    }
}

struct Frame {
    sinim: f64,
    cosim: f64,
    sinomm: f64,
    cosomm: f64,
    em: f64,
    emsq: f64,
    rtemsq: f64,
    xnoi: f64,
}

impl Frame {
    #[allow(clippy::too_many_arguments)]
    fn body(
        &self,
        zcosg: f64,
        zsing: f64,
        zcosi: f64,
        zsini: f64,
        zcosh: f64,
        zsinh: f64,
        cc: f64,
    ) -> Body {
        let (sinim, cosim, emsq) = (self.sinim, self.cosim, self.emsq);
        let a1 = zcosg * zcosh + zsing * zcosi * zsinh;
        let a3 = -zsing * zcosh + zcosg * zcosi * zsinh;
        let a7 = -zcosg * zsinh + zsing * zcosi * zcosh;
        let a8 = zsing * zsini;
        let a9 = zsing * zsinh + zcosg * zcosi * zcosh;
        let a10 = zcosg * zsini;
        let a2 = cosim * a7 + sinim * a8;
        let a4 = cosim * a9 + sinim * a10;
        let a5 = -sinim * a7 + cosim * a8;
        let a6 = -sinim * a9 + cosim * a10;
        let x1 = a1 * self.cosomm + a2 * self.sinomm;
        let x2 = a3 * self.cosomm + a4 * self.sinomm;
        let x3 = -a1 * self.sinomm + a2 * self.cosomm;
        let x4 = -a3 * self.sinomm + a4 * self.cosomm;
        let x5 = a5 * self.sinomm;
        let x6 = a6 * self.sinomm;
        let x7 = a5 * self.cosomm;
        let x8 = a6 * self.cosomm;
        let z31 = 12.0 * x1 * x1 - 3.0 * x3 * x3;
        let z32 = 24.0 * x1 * x2 - 6.0 * x3 * x4;
        let z33 = 12.0 * x2 * x2 - 3.0 * x4 * x4;
        let betasq = 1.0 - emsq;
        let z1 = 3.0 * (a1 * a1 + a2 * a2) + z31 * emsq;
        let z2 = 6.0 * (a1 * a3 + a2 * a4) + z32 * emsq;
        let z3 = 3.0 * (a3 * a3 + a4 * a4) + z33 * emsq;
        let s3 = cc * self.xnoi;
        let s4 = s3 * self.rtemsq;
        Body {
            s: [
                -15.0 * self.em * s4,
                -0.5 * s3 / self.rtemsq,
                s3,
                s4,
                x1 * x3 + x2 * x4,
                x2 * x3 + x1 * x4,
                x2 * x4 - x1 * x3,
            ],
            z1: z1 + z1 + betasq * z31,
            z2: z2 + z2 + betasq * z32,
            z3: z3 + z3 + betasq * z33,
            z11: -6.0 * a1 * a5 + emsq * (-24.0 * x1 * x7 - 6.0 * x3 * x5),
            z12: -6.0 * (a1 * a6 + a3 * a5)
                + emsq * (-24.0 * (x2 * x7 + x1 * x8) - 6.0 * (x3 * x6 + x4 * x5)),
            z13: -6.0 * a3 * a6 + emsq * (-24.0 * x2 * x8 - 6.0 * x4 * x6),
            z21: 6.0 * a2 * a5 + emsq * (24.0 * x1 * x5 - 6.0 * x3 * x7),
            z22: 6.0 * (a4 * a5 + a2 * a6)
                + emsq * (24.0 * (x2 * x5 + x1 * x6) - 6.0 * (x4 * x7 + x3 * x8)),
            z23: 6.0 * a4 * a6 + emsq * (24.0 * x2 * x6 - 6.0 * x4 * x8),
            z31,
            z32,
            z33,
        }
    }
}

fn periodic(body: &Body, emsq: f64, eccentricity: f64) -> Periodic {
    let [s1, s2, s3, s4, _, s6, s7] = body.s;
    Periodic {
        e2: 2.0 * s1 * s6,
        e3: 2.0 * s1 * s7,
        i2: 2.0 * s2 * body.z12,
        i3: 2.0 * s2 * (body.z13 - body.z11),
        l2: -2.0 * s3 * body.z2,
        l3: -2.0 * s3 * (body.z3 - body.z1),
        l4: -2.0 * s3 * (-21.0 - 9.0 * emsq) * eccentricity,
        gh2: 2.0 * s4 * body.z32,
        gh3: 2.0 * s4 * (body.z33 - body.z31),
        gh4: -18.0 * s4 * eccentricity,
        h2: -2.0 * s2 * body.z22,
        h3: -2.0 * s2 * (body.z23 - body.z21),
    }
}

fn half_day_terms(shape: &Shape, em: f64, emsq: f64, nm: f64, aonv: f64) -> [f64; 10] {
    let (sinim, cosim) = (shape.sinio, shape.cosio);
    let cosisq = cosim * cosim;
    let eoc = em * emsq;
    let g201 = -0.306 - (em - 0.64) * 0.440;
    let (g211, g310, g322, g410, g422, g520) = if em <= 0.65 {
        (
            3.616 - 13.2470 * em + 16.2900 * emsq,
            -19.302 + 117.3900 * em - 228.4190 * emsq + 156.5910 * eoc,
            -18.9068 + 109.7927 * em - 214.6334 * emsq + 146.5816 * eoc,
            -41.122 + 242.6940 * em - 471.0940 * emsq + 313.9530 * eoc,
            -146.407 + 841.8800 * em - 1629.014 * emsq + 1083.4350 * eoc,
            -532.114 + 3017.977 * em - 5740.032 * emsq + 3708.2760 * eoc,
        )
    } else {
        (
            -72.099 + 331.819 * em - 508.738 * emsq + 266.724 * eoc,
            -346.844 + 1582.851 * em - 2415.925 * emsq + 1246.113 * eoc,
            -342.585 + 1554.908 * em - 2366.899 * emsq + 1215.972 * eoc,
            -1052.797 + 4758.686 * em - 7193.992 * emsq + 3651.957 * eoc,
            -3581.690 + 16178.110 * em - 24462.770 * emsq + 12422.520 * eoc,
            if em > 0.715 {
                -5149.66 + 29936.92 * em - 54087.36 * emsq + 31324.56 * eoc
            } else {
                1464.74 - 4664.75 * em + 3763.64 * emsq
            },
        )
    };
    let (g533, g521, g532) = if em < 0.7 {
        (
            -919.22770 + 4988.6100 * em - 9064.7700 * emsq + 5542.21 * eoc,
            -822.71072 + 4568.6173 * em - 8491.4146 * emsq + 5337.524 * eoc,
            -853.66600 + 4690.2500 * em - 8624.7700 * emsq + 5341.4 * eoc,
        )
    } else {
        (
            -37995.780 + 161616.52 * em - 229838.20 * emsq + 109377.94 * eoc,
            -51752.104 + 218913.95 * em - 309468.16 * emsq + 146349.42 * eoc,
            -40023.880 + 170470.89 * em - 242699.48 * emsq + 115605.82 * eoc,
        )
    };
    let sini2 = sinim * sinim;
    let f220 = 0.75 * (1.0 + 2.0 * cosim + cosisq);
    let f221 = 1.5 * sini2;
    let f321 = 1.875 * sinim * (1.0 - 2.0 * cosim - 3.0 * cosisq);
    let f322 = -1.875 * sinim * (1.0 + 2.0 * cosim - 3.0 * cosisq);
    let f441 = 35.0 * sini2 * f220;
    let f442 = 39.3750 * sini2 * sini2;
    let f522 = 9.843_75
        * sinim
        * (sini2 * (1.0 - 2.0 * cosim - 5.0 * cosisq)
            + 0.333_333_33 * (-2.0 + 4.0 * cosim + 6.0 * cosisq));
    let f523 = sinim
        * (4.921_875_12 * sini2 * (-2.0 - 4.0 * cosim + 10.0 * cosisq)
            + 6.562_500_12 * (1.0 + 2.0 * cosim - 3.0 * cosisq));
    let f542 =
        29.531_25 * sinim * (2.0 - 8.0 * cosim + cosisq * (-12.0 + 8.0 * cosim + 10.0 * cosisq));
    let f543 =
        29.531_25 * sinim * (-2.0 - 8.0 * cosim + cosisq * (12.0 + 8.0 * cosim - 10.0 * cosisq));
    let mut temp1 = 3.0 * nm * nm * aonv * aonv;
    let t22 = temp1 * 1.789_167_9e-6;
    temp1 *= aonv;
    let t32 = temp1 * 3.739_379_2e-7;
    temp1 *= aonv;
    let t44 = 2.0 * temp1 * 7.363_695_3e-9;
    temp1 *= aonv;
    let t52 = temp1 * 1.142_863_9e-7;
    let t54 = 2.0 * temp1 * 2.176_580_3e-9;
    [
        t22 * f220 * g201,
        t22 * f221 * g211,
        t32 * f321 * g310,
        t32 * f322 * g322,
        t44 * f441 * g410,
        t44 * f442 * g422,
        t52 * f522 * g520,
        t52 * f523 * g532,
        t54 * f542 * g521,
        t54 * f543 * g533,
    ]
}
