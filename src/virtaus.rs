//! Tiheä optinen virtaus (DIS-periaate, Kroeger ym. 2016): pyramidi karkeasta tarkkaan,
//! laikkujen inverse-compositional-haku ja tiheäksi levitys fotometrisellä painotuksella.
//! Palautettu virtaus v toteuttaa I0(x) ≈ I1(x + v(x)).

use rayon::prelude::*;

use crate::kuva::{Kentta, Taso};

const LAIKKU: usize = 8;
const ASKEL: usize = 4;
const KIERROKSET: usize = 12;

fn pyramidi(k: &Taso, tasoja: usize) -> Vec<Taso> {
    let mut v = vec![k.clone()];
    for _ in 1..tasoja {
        let e = v.last().unwrap();
        v.push(e.koon((e.w / 2).max(1), (e.h / 2).max(1)));
    }
    v
}

fn gradientit(k: &Taso) -> (Taso, Taso) {
    let (w, h) = (k.w, k.h);
    let mut gx = Taso::new(w, h);
    let mut gy = Taso::new(w, h);
    gx.data.par_chunks_mut(w).zip(gy.data.par_chunks_mut(w)).enumerate().for_each(|(y, (rx, ry))| {
        let ym = y.saturating_sub(1);
        let yp = (y + 1).min(h - 1);
        for x in 0..w {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(w - 1);
            rx[x] = (k.get(xp, y) - k.get(xm, y)) * 0.5;
            ry[x] = (k.get(x, yp) - k.get(x, ym)) * 0.5;
        }
    });
    (gx, gy)
}

fn ruudukko(n: usize) -> Vec<usize> {
    if n <= LAIKKU {
        return vec![0];
    }
    let mut v: Vec<usize> = (0..=n - LAIKKU).step_by(ASKEL).collect();
    if *v.last().unwrap() != n - LAIKKU {
        v.push(n - LAIKKU);
    }
    v
}

/// Yhden pyramiditason laskenta alkuarvauksesta `alku`.
fn taso(i0: &Taso, i1: &Taso, alku: &Kentta) -> Kentta {
    let (w, h) = (i0.w, i0.h);
    if w < LAIKKU || h < LAIKKU {
        return alku.clone();
    }
    let (gx, gy) = gradientit(i0);
    let xs = ruudukko(w);
    let ys = ruudukko(h);
    let ps = LAIKKU as f32;
    // laikkujen virtaukset
    let laikut: Vec<(f32, f32)> = ys
        .par_iter()
        .flat_map_iter(|&y0| xs.iter().map(move |&x0| (x0, y0)))
        .map(|(x0, y0)| {
            let (cx, cy) = (x0 + LAIKKU / 2, y0 + LAIKKU / 2);
            let (ux0, uy0) = (alku.x.get(cx.min(w - 1), cy.min(h - 1)), alku.y.get(cx.min(w - 1), cy.min(h - 1)));
            let mut t = [0.0f32; LAIKKU * LAIKKU];
            let mut tx = [0.0f32; LAIKKU * LAIKKU];
            let mut ty = [0.0f32; LAIKKU * LAIKKU];
            let (mut hxx, mut hxy, mut hyy, mut t_ka) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for j in 0..LAIKKU {
                for i in 0..LAIKKU {
                    let k = j * LAIKKU + i;
                    t[k] = i0.get(x0 + i, y0 + j);
                    tx[k] = gx.get(x0 + i, y0 + j);
                    ty[k] = gy.get(x0 + i, y0 + j);
                    hxx += tx[k] * tx[k];
                    hxy += tx[k] * ty[k];
                    hyy += ty[k] * ty[k];
                    t_ka += t[k];
                }
            }
            t_ka /= (LAIKKU * LAIKKU) as f32;
            let det = hxx * hyy - hxy * hxy;
            if det < 1e-2 {
                return (ux0, uy0);
            }
            let (ixx, ixy, iyy) = (hyy / det, -hxy / det, hxx / det);
            let (mut ux, mut uy) = (ux0, uy0);
            for _ in 0..KIERROKSET {
                let mut nayte = [0.0f32; LAIKKU * LAIKKU];
                let mut n_ka = 0.0;
                for j in 0..LAIKKU {
                    for i in 0..LAIKKU {
                        let v = i1.nayte((x0 + i) as f32 + ux, (y0 + j) as f32 + uy);
                        nayte[j * LAIKKU + i] = v;
                        n_ka += v;
                    }
                }
                n_ka /= (LAIKKU * LAIKKU) as f32;
                let (mut bx, mut by) = (0.0f32, 0.0f32);
                for k in 0..LAIKKU * LAIKKU {
                    let e = (nayte[k] - n_ka) - (t[k] - t_ka);
                    bx += tx[k] * e;
                    by += ty[k] * e;
                }
                let dx = ixx * bx + ixy * by;
                let dy = ixy * bx + iyy * by;
                ux -= dx;
                uy -= dy;
                if dx * dx + dy * dy < 1e-4 {
                    break;
                }
            }
            if (ux - ux0).hypot(uy - uy0) > ps {
                (ux0, uy0)
            } else {
                (ux, uy)
            }
        })
        .collect();
    // tiheäksi levitys: pikselin virtaus = peittävien laikkujen painotettu keskiarvo
    let nx = xs.len();
    let mut ulos = Kentta::new(w, h);
    ulos.x.data.par_chunks_mut(w).zip(ulos.y.data.par_chunks_mut(w)).enumerate().for_each(|(y, (rx, ry))| {
        let jy: Vec<usize> = ys.iter().enumerate().filter(|&(_, &y0)| y >= y0 && y < y0 + LAIKKU).map(|(j, _)| j).collect();
        let mut ix_alku = 0usize;
        for x in 0..w {
            while ix_alku < nx && xs[ix_alku] + LAIKKU <= x {
                ix_alku += 1;
            }
            let (mut sx, mut sy, mut sp) = (0.0f32, 0.0f32, 0.0f32);
            let i0v = i0.get(x, y);
            for &j in &jy {
                let mut i = ix_alku;
                while i < nx && xs[i] <= x {
                    let (ux, uy) = laikut[j * nx + i];
                    let e = (i1.nayte(x as f32 + ux, y as f32 + uy) - i0v).abs();
                    let p = 1.0 / e.max(1.0);
                    sx += p * ux;
                    sy += p * uy;
                    sp += p;
                    i += 1;
                }
            }
            if sp > 0.0 {
                rx[x] = sx / sp;
                ry[x] = sy / sp;
            } else {
                rx[x] = alku.x.get(x, y);
                ry[x] = alku.y.get(x, y);
            }
        }
    });
    ulos.gauss(1.0)
}

/// Virtaus kuvasta i0 kuvaan i1 (samankokoiset harmaakuvat 0..255).
pub fn virtaus(i0: &Taso, i1: &Taso) -> Kentta {
    let (w, h) = (i0.w, i0.h);
    let mut tasoja = 1;
    while (w >> tasoja) >= 24 && (h >> tasoja) >= 24 {
        tasoja += 1;
    }
    let p0 = pyramidi(i0, tasoja);
    let p1 = pyramidi(i1, tasoja);
    // tarkin laskettava taso on puolikas (kuten DIS:n medium-asetus); lopuksi suurennetaan
    let tarkin = if tasoja > 1 { 1 } else { 0 };
    let karkein = &p0[tasoja - 1];
    let mut v = Kentta::new(karkein.w, karkein.h);
    for t in (tarkin..tasoja).rev() {
        let (tw, th) = (p0[t].w, p0[t].h);
        if v.w() != tw || v.h() != th {
            let s = tw as f32 / v.w() as f32;
            v = v.koon(tw, th);
            v.x.data.iter_mut().for_each(|x| *x *= s);
            v.y.data.iter_mut().for_each(|y| *y *= s);
        }
        v = taso(&p0[t], &p1[t], &v);
    }
    if v.w() != w || v.h() != h {
        let s = w as f32 / v.w() as f32;
        v = v.koon(w, h);
        v.x.data.iter_mut().for_each(|x| *x *= s);
        v.y.data.iter_mut().for_each(|y| *y *= s);
    }
    v
}

/// Kenttä tai kuva näytteistetään kohdasta x + v(x). `v` voi olla eri kokoinen; se skaalataan.
pub fn kohdista(k: &Taso, v: &Kentta) -> Taso {
    let (w, h) = (k.w, k.h);
    let v = if v.w() != w || v.h() != h {
        let s = w as f32 / v.w() as f32;
        let mut u = v.koon(w, h);
        u.x.data.iter_mut().for_each(|x| *x *= s);
        u.y.data.iter_mut().for_each(|y| *y *= s);
        std::borrow::Cow::Owned(u)
    } else {
        std::borrow::Cow::Borrowed(v)
    };
    let mut ulos = Taso::new(w, h);
    ulos.data.par_chunks_mut(w).enumerate().for_each(|(y, r)| {
        for x in 0..w {
            let i = y * w + x;
            r[x] = k.nayte(x as f32 + v.x.data[i], y as f32 + v.y.data[i]);
        }
    });
    ulos
}

pub fn kohdista_kentta(k: &Kentta, v: &Kentta) -> Kentta {
    let (a, b) = rayon::join(|| kohdista(&k.x, v), || kohdista(&k.y, v));
    Kentta { x: a, y: b }
}

/// Ketjutus: v(i->j) = v(i->j-1) + askel(j-1->j) näytteistettynä kohdasta x + v(i->j-1).
pub fn ketjuta(v: &Kentta, askel: &Kentta) -> Kentta {
    let a = kohdista_kentta(askel, v);
    let mut u = v.clone();
    u.x.data.par_iter_mut().zip(&a.x.data).for_each(|(x, d)| *x += d);
    u.y.data.par_iter_mut().zip(&a.y.data).for_each(|(y, d)| *y += d);
    u
}
