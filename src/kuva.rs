//! Kuvatyypit ja perusoperaatiot: koon muutos, sumennus, morfologia, yhtenäiset alueet.

use rayon::prelude::*;

/// 8-bittinen BGR-kuva (ffmpegin bgr24-järjestys).
#[derive(Clone)]
pub struct Bgr {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Bgr {
    pub fn new(w: usize, h: usize) -> Self {
        Bgr { w, h, data: vec![0; w * h * 3] }
    }

    pub fn koon(&self, w: usize, h: usize) -> Bgr {
        if w == self.w && h == self.h {
            return self.clone();
        }
        let mut kanavat = [Taso::new(self.w, self.h), Taso::new(self.w, self.h), Taso::new(self.w, self.h)];
        for (i, p) in self.data.chunks_exact(3).enumerate() {
            for c in 0..3 {
                kanavat[c].data[i] = p[c] as f32;
            }
        }
        let k: Vec<Taso> = kanavat.par_iter().map(|t| t.koon(w, h)).collect();
        let mut ulos = Bgr::new(w, h);
        for i in 0..w * h {
            for c in 0..3 {
                ulos.data[i * 3 + c] = k[c].data[i].round().clamp(0.0, 255.0) as u8;
            }
        }
        ulos
    }

    /// Harmaa (0.299 R + 0.587 G + 0.114 B), kuten OpenCV:n BGR2GRAY.
    pub fn harmaa(&self) -> Vec<u8> {
        self.data
            .chunks_exact(3)
            .map(|p| (0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32).round() as u8)
            .collect()
    }

    pub fn rgb(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.data.len());
        for p in self.data.chunks_exact(3) {
            v.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        v
    }

    pub fn rgb_kuvasta(w: usize, h: usize, rgb: &[u8]) -> Bgr {
        let mut data = Vec::with_capacity(w * h * 3);
        for p in rgb.chunks_exact(3) {
            data.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        Bgr { w, h, data }
    }

    pub fn vierekkain(kuvat: &[&Bgr]) -> Bgr {
        let h = kuvat.iter().map(|k| k.h).max().unwrap_or(0);
        let w: usize = kuvat.iter().map(|k| k.w).sum();
        let mut ulos = Bgr::new(w, h);
        let mut x0 = 0;
        for k in kuvat {
            for y in 0..k.h {
                let a = (y * w + x0) * 3;
                ulos.data[a..a + k.w * 3].copy_from_slice(&k.data[y * k.w * 3..(y + 1) * k.w * 3]);
            }
            x0 += k.w;
        }
        ulos
    }
}

/// Yksikanavainen float-taso.
#[derive(Clone)]
pub struct Taso {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Taso {
    pub fn new(w: usize, h: usize) -> Self {
        Taso { w, h, data: vec![0.0; w * h] }
    }
    pub fn u8sta(w: usize, h: usize, d: &[u8]) -> Self {
        Taso { w, h, data: d.iter().map(|&x| x as f32).collect() }
    }
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.w + x]
    }
    pub fn keskiarvo(&self) -> f32 {
        (self.data.iter().map(|&v| v as f64).sum::<f64>() / self.data.len().max(1) as f64) as f32
    }

    /// Bilineaarinen näyte (reunat toistetaan).
    #[inline]
    pub fn nayte(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let x0 = x as usize;
        let y0 = y as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let r0 = self.data[y0 * self.w + x0] * (1.0 - fx) + self.data[y0 * self.w + x1] * fx;
        let r1 = self.data[y1 * self.w + x0] * (1.0 - fx) + self.data[y1 * self.w + x1] * fx;
        r0 * (1.0 - fy) + r1 * fy
    }

    /// Koon muutos: pienennys pinta-alakeskiarvolla (INTER_AREA), suurennus bilineaarisesti.
    pub fn koon(&self, w: usize, h: usize) -> Taso {
        if w == self.w && h == self.h {
            return self.clone();
        }
        let vaaka = painot(self.w, w);
        let pysty = painot(self.h, h);
        // ensin vaakasuunta
        let mut valivaihe = vec![0.0f32; w * self.h];
        valivaihe.par_chunks_mut(w).enumerate().for_each(|(y, rivi)| {
            let lahde = &self.data[y * self.w..(y + 1) * self.w];
            for (x, p) in vaaka.iter().enumerate() {
                rivi[x] = p.iter().map(|&(i, k)| lahde[i] * k).sum();
            }
        });
        let mut ulos = Taso::new(w, h);
        ulos.data.par_chunks_mut(w).enumerate().for_each(|(y, rivi)| {
            for &(i, k) in &pysty[y] {
                let l = &valivaihe[i * w..(i + 1) * w];
                for x in 0..w {
                    rivi[x] += l[x] * k;
                }
            }
        });
        ulos
    }

    pub fn gauss(&self, sigma: f32) -> Taso {
        if sigma <= 0.0 {
            return self.clone();
        }
        let r = (sigma * 3.0).ceil() as i32;
        let mut k: Vec<f32> = (-r..=r).map(|i| (-0.5 * (i as f32 / sigma).powi(2)).exp()).collect();
        let s: f32 = k.iter().sum();
        k.iter_mut().for_each(|v| *v /= s);
        self.erotettava(&k, r)
    }

    /// Laatikkosumennus (kuten cv2.blur, koko n x n).
    pub fn laatikko(&self, n: usize) -> Taso {
        let r = (n / 2) as i32;
        let k = vec![1.0 / n as f32; n];
        self.erotettava(&k, r)
    }

    fn erotettava(&self, k: &[f32], r: i32) -> Taso {
        let (w, h) = (self.w, self.h);
        let mut v = vec![0.0f32; w * h];
        v.par_chunks_mut(w).enumerate().for_each(|(y, rivi)| {
            let l = &self.data[y * w..(y + 1) * w];
            for x in 0..w {
                let mut s = 0.0;
                for (j, &kk) in k.iter().enumerate() {
                    s += l[heijasta(x as i32 + j as i32 - r, w)] * kk;
                }
                rivi[x] = s;
            }
        });
        let mut ulos = Taso::new(w, h);
        ulos.data.par_chunks_mut(w).enumerate().for_each(|(y, rivi)| {
            for (j, &kk) in k.iter().enumerate() {
                let yy = heijasta(y as i32 + j as i32 - r, h);
                let l = &v[yy * w..(yy + 1) * w];
                for x in 0..w {
                    rivi[x] += l[x] * kk;
                }
            }
        });
        ulos
    }
}

#[inline]
fn heijasta(i: i32, n: usize) -> usize {
    // BORDER_REFLECT_101
    let n = n as i32;
    if n == 1 {
        return 0;
    }
    let mut i = i;
    while i < 0 || i >= n {
        if i < 0 {
            i = -i;
        }
        if i >= n {
            i = 2 * n - 2 - i;
        }
    }
    i as usize
}

/// Uudelleennäytteistyksen painot yhdelle akselille.
fn painot(lahde: usize, kohde: usize) -> Vec<Vec<(usize, f32)>> {
    let s = lahde as f32 / kohde as f32;
    (0..kohde)
        .map(|o| {
            if s > 1.0 {
                // pinta-alakeskiarvo
                let a = o as f32 * s;
                let b = a + s;
                let mut v = Vec::new();
                let mut i = a.floor() as usize;
                while (i as f32) < b && i < lahde {
                    let osuus = (b.min(i as f32 + 1.0) - a.max(i as f32)).max(0.0);
                    if osuus > 0.0 {
                        v.push((i, osuus / s));
                    }
                    i += 1;
                }
                v
            } else {
                let x = ((o as f32 + 0.5) * s - 0.5).max(0.0);
                let x0 = (x as usize).min(lahde - 1);
                let x1 = (x0 + 1).min(lahde - 1);
                let f = x - x0 as f32;
                if x1 == x0 { vec![(x0, 1.0)] } else { vec![(x0, 1.0 - f), (x1, f)] }
            }
        })
        .collect()
}

/// Kaksikanavainen float-kenttä (a/b tai optinen virtaus), kanavat erikseen.
#[derive(Clone)]
pub struct Kentta {
    pub x: Taso,
    pub y: Taso,
}

impl Kentta {
    pub fn new(w: usize, h: usize) -> Self {
        Kentta { x: Taso::new(w, h), y: Taso::new(w, h) }
    }
    pub fn w(&self) -> usize {
        self.x.w
    }
    pub fn h(&self) -> usize {
        self.x.h
    }
    pub fn koon(&self, w: usize, h: usize) -> Kentta {
        if w == self.w() && h == self.h() {
            return self.clone();
        }
        let (a, b) = rayon::join(|| self.x.koon(w, h), || self.y.koon(w, h));
        Kentta { x: a, y: b }
    }
    pub fn gauss(&self, s: f32) -> Kentta {
        let (a, b) = rayon::join(|| self.x.gauss(s), || self.y.gauss(s));
        Kentta { x: a, y: b }
    }
}

// --------------------------------------------------------------------------
// Binäärimaskit
// --------------------------------------------------------------------------
pub fn laajenna(m: &[u8], w: usize, h: usize, n: usize, max: bool) -> Vec<u8> {
    let r = (n / 2) as i32;
    let valitse = |a: u8, b: u8| if max { a.max(b) } else { a.min(b) };
    let mut v = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = if max { 0 } else { 255 };
            for d in -r..=r {
                let xx = (x as i32 + d).clamp(0, w as i32 - 1) as usize;
                s = valitse(s, m[y * w + xx]);
            }
            v[y * w + x] = s;
        }
    }
    let mut u = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = if max { 0 } else { 255 };
            for d in -r..=r {
                let yy = (y as i32 + d).clamp(0, h as i32 - 1) as usize;
                s = valitse(s, v[yy * w + x]);
            }
            u[y * w + x] = s;
        }
    }
    u
}

pub struct Alue {
    pub pinta: usize,
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

/// Yhtenäiset alueet (8-naapuruus). Palauttaa nimiökartan (0 = tausta) ja alueet (indeksi = nimiö - 1).
pub fn alueet(m: &[u8], w: usize, h: usize) -> (Vec<u32>, Vec<Alue>) {
    let mut nimio = vec![0u32; w * h];
    let mut lista = Vec::new();
    let mut pino = Vec::new();
    for i in 0..w * h {
        if m[i] == 0 || nimio[i] != 0 {
            continue;
        }
        let n = lista.len() as u32 + 1;
        let mut a = Alue { pinta: 0, x0: usize::MAX, y0: usize::MAX, x1: 0, y1: 0 };
        nimio[i] = n;
        pino.push(i);
        while let Some(j) = pino.pop() {
            let (x, y) = (j % w, j / w);
            a.pinta += 1;
            a.x0 = a.x0.min(x);
            a.y0 = a.y0.min(y);
            a.x1 = a.x1.max(x);
            a.y1 = a.y1.max(y);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                        continue;
                    }
                    let k = yy as usize * w + xx as usize;
                    if m[k] != 0 && nimio[k] == 0 {
                        nimio[k] = n;
                        pino.push(k);
                    }
                }
            }
        }
        lista.push(a);
    }
    (nimio, lista)
}
