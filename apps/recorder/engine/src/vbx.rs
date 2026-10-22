// VBx clustering, as pyannote's community-1 does it (pyannote.audio's VBxClustering, after BUT's VBx, Apache-2.0):
// the clean-speech embeddings are first grouped by centroid-linkage at a low threshold (more groups than voices, on
// purpose), then a PLDA turns each embedding into features where one voice varies little and two voices differ a lot,
// and variational Bayes decides which groups are one person: groups with no evidence of their own fade out (their
// prior goes to zero). What it changes from plain linkage is the count: fewer voices split in two, fewer pairs merged.
//
// The PLDA (pyannote/speaker-diarization-community-1, CC BY 4.0; weights/README.md) is built into the binary, as
// eval/plda.py converts it: its eigendecomposition is done there, here are only products.
use anlg_pyannote::clustering;

/// pyannote's VBxClustering defaults (community-1).
const THRESHOLD: f32 = 0.6;
const FA: f64 = 0.07;
const FB: f64 = 0.8;
const MAX_ITERS: usize = 20;
const INIT_SMOOTHING: f64 = 7.0;
const MIN_PRIOR: f64 = 1e-7;

pub struct Plda {
    mean1: Vec<f64>,
    lda: Vec<Vec<f64>>,
    mean2: Vec<f64>,
    mu: Vec<f64>,
    tr: Vec<Vec<f64>>,
    phi: Vec<f64>,
}

static PLDA: std::sync::LazyLock<Plda> = std::sync::LazyLock::new(|| Plda::parse(include_bytes!("weights/plda.bin")));

impl Plda {
    /// eval/plda.py's layout: in, lda, features (u32), then mean1, lda, mean2, mu, tr, phi (f32), little-endian.
    fn parse(b: &[u8]) -> Plda {
        let u = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()) as usize;
        let (din, dlda, d) = (u(0), u(1), u(2));
        let mut at = 12;
        let mut take = |n: usize| -> Vec<f64> {
            let v = b[at..at + n * 4].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap()) as f64).collect();
            at += n * 4;
            v
        };
        let mean1 = take(din);
        let lda = take(din * dlda).chunks(dlda).map(<[f64]>::to_vec).collect();
        let mean2 = take(dlda);
        let mu = take(dlda);
        let tr = take(d * dlda).chunks(dlda).map(<[f64]>::to_vec).collect();
        let phi = take(d);
        assert_eq!(at, b.len(), "weights/plda.bin isn't eval/plda.py's layout");
        Plda { mean1, lda, mean2, mu, tr, phi }
    }

    /// An embedding as PLDA features (pyannote's xvec_tf, then plda_tf).
    fn features(&self, x: &[f32]) -> Vec<f64> {
        let (din, dlda) = (self.lda.len(), self.mean2.len());
        let centered: Vec<f64> = x.iter().zip(&self.mean1).map(|(x, m)| *x as f64 - m).collect();
        let v = scaled_unit(&centered, (din as f64).sqrt());
        let mut y = vec![0.0; dlda];
        for (i, row) in self.lda.iter().enumerate() {
            for (j, w) in row.iter().enumerate() {
                y[j] += v[i] * w;
            }
        }
        let y: Vec<f64> = y.iter().zip(&self.mean2).map(|(y, m)| y - m).collect();
        let y = scaled_unit(&y, (dlda as f64).sqrt());
        let c: Vec<f64> = y.iter().zip(&self.mu).map(|(y, m)| y - m).collect();
        self.tr.iter().map(|row| row.iter().zip(&c).map(|(t, c)| t * c).sum()).collect()
    }
}

fn scaled_unit(v: &[f64], scale: f64) -> Vec<f64> {
    let n = v.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
    v.iter().map(|x| x / n * scale).collect()
}

/// A voice for each of `embeddings` (the anchors): linkage at THRESHOLD, refined by VBx. Voices numbered from 0.
pub fn cluster(embeddings: &[Vec<f32>]) -> Vec<usize> {
    let plda = &*PLDA;
    let n = embeddings.len();
    if n < 2 {
        return vec![0; n];
    }
    let init = clustering::cluster(embeddings, THRESHOLD, 1, clustering::SpeakerBounds::default());
    let k = init.iter().max().map_or(1, |m| m + 1);
    if k == 1 {
        return init;
    }
    let x: Vec<Vec<f64>> = embeddings.iter().map(|e| plda.features(e)).collect();
    // Soft start from the linkage groups: softmax(one-hot × 7), as pyannote.
    let mut gamma: Vec<Vec<f64>> = init.iter().map(|&l| softmax(&(0..k).map(|j| if j == l { INIT_SMOOTHING } else { 0.0 }).collect::<Vec<_>>())).collect();
    let pi = vbx(&x, &plda.phi, &mut gamma);
    let kept: Vec<usize> = (0..k).filter(|&j| pi[j] > MIN_PRIOR).collect();
    // Each embedding to its most likely kept voice.
    let labels: Vec<usize> = gamma.iter().map(|g| kept.iter().copied().max_by(|a, b| g[*a].total_cmp(&g[*b])).unwrap_or(0)).collect();
    renumber(&labels)
}

/// Variational Bayes over the voices (BUT's VBx without the HMM, as pyannote runs it). `gamma`: each embedding's
/// responsibility per voice, refined in place. Returns the voices' priors.
fn vbx(x: &[Vec<f64>], phi: &[f64], gamma: &mut [Vec<f64>]) -> Vec<f64> {
    let (n, k, d) = (x.len(), gamma[0].len(), phi.len());
    let mut pi = vec![1.0 / k as f64; k];
    let g: Vec<f64> = x.iter().map(|r| -0.5 * (r.iter().map(|v| v * v).sum::<f64>() + d as f64 * (2.0 * std::f64::consts::PI).ln())).collect();
    let v: Vec<f64> = phi.iter().map(|p| p.sqrt()).collect();
    let rho: Vec<Vec<f64>> = x.iter().map(|r| r.iter().zip(&v).map(|(a, b)| a * b).collect()).collect();
    let mut last_elbo = f64::NEG_INFINITY;
    for _ in 0..MAX_ITERS {
        // Speaker models: inv_l[s][j], alpha[s][j].
        let counts: Vec<f64> = (0..k).map(|s| gamma.iter().map(|r| r[s]).sum()).collect();
        let inv_l: Vec<Vec<f64>> = counts.iter().map(|c| phi.iter().map(|p| 1.0 / (1.0 + FA / FB * c * p)).collect()).collect();
        let alpha: Vec<Vec<f64>> = (0..k)
            .map(|s| (0..d).map(|j| FA / FB * inv_l[s][j] * (0..n).map(|t| gamma[t][s] * rho[t][j]).sum::<f64>()).collect())
            .collect();
        // log p(x_t | s)
        let quad: Vec<f64> = (0..k).map(|s| (0..d).map(|j| (inv_l[s][j] + alpha[s][j] * alpha[s][j]) * phi[j]).sum()).collect();
        let lpi: Vec<f64> = pi.iter().map(|p| (p + 1e-8).ln()).collect();
        let mut log_px_total = 0.0;
        for t in 0..n {
            let lp: Vec<f64> = (0..k)
                .map(|s| FA * (rho[t].iter().zip(&alpha[s]).map(|(r, a)| r * a).sum::<f64>() - 0.5 * quad[s] + g[t]) + lpi[s])
                .collect();
            let m = lp.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let lse = m + lp.iter().map(|l| (l - m).exp()).sum::<f64>().ln();
            log_px_total += lse;
            for s in 0..k {
                gamma[t][s] = (lp[s] - lse).exp();
            }
        }
        let sums: Vec<f64> = (0..k).map(|s| gamma.iter().map(|r| r[s]).sum()).collect();
        let total: f64 = sums.iter().sum::<f64>().max(1e-12);
        pi = sums.iter().map(|s| s / total).collect();
        let elbo = log_px_total
            + FB * 0.5 * (0..k).map(|s| (0..d).map(|j| inv_l[s][j].ln() - inv_l[s][j] - alpha[s][j] * alpha[s][j] + 1.0).sum::<f64>()).sum::<f64>();
        if elbo - last_elbo < 1e-6 {
            break;
        }
        last_elbo = elbo;
    }
    pi
}

fn softmax(v: &[f64]) -> Vec<f64> {
    let m = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let e: Vec<f64> = v.iter().map(|x| (x - m).exp()).collect();
    let s: f64 = e.iter().sum();
    e.iter().map(|x| x / s).collect()
}

fn renumber(labels: &[usize]) -> Vec<usize> {
    let mut order: Vec<usize> = vec![];
    labels
        .iter()
        .map(|l| {
            if !order.contains(l) {
                order.push(*l);
            }
            order.iter().position(|x| x == l).unwrap()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn plda_is_built_in() {
        let p = &*super::PLDA;
        assert_eq!((p.mean1.len(), p.mean2.len(), p.phi.len()), (256, 128, 128));
        assert!(p.phi.iter().all(|v| v.is_finite() && *v > 0.0));
        assert_eq!(p.features(&[0.1; 256]).len(), 128);
    }

    #[test]
    fn softmax_sums_to_one() {
        let s = super::softmax(&[7.0, 0.0, 0.0]);
        assert!((s.iter().sum::<f64>() - 1.0).abs() < 1e-12 && s[0] > 0.99);
    }

    /// Two clear groups in feature space stay two; a third, tiny group with no evidence of its own fades out.
    #[test]
    fn vbx_keeps_supported_voices() {
        let phi = vec![2.0; 4];
        let a = vec![3.0, 0.0, 0.0, 0.0];
        let b = vec![0.0, 3.0, 0.0, 0.0];
        let mut x = vec![];
        for i in 0..20 {
            let e = (i as f64) * 0.01;
            x.push(a.iter().map(|v| v + e).collect::<Vec<_>>());
            x.push(b.iter().map(|v| v + e).collect::<Vec<_>>());
        }
        // Linkage put one embedding of voice a in a group of its own.
        let mut gamma: Vec<Vec<f64>> = (0..x.len()).map(|t| super::softmax(&[if t % 2 == 0 && t != 0 { 7.0 } else { 0.0 }, if t % 2 == 1 { 7.0 } else { 0.0 }, if t == 0 { 7.0 } else { 0.0 }])).collect();
        let pi = super::vbx(&x, &phi, &mut gamma);
        assert!(pi[0] > 0.3 && pi[1] > 0.3, "{pi:?}");
        assert!(gamma[0][0] > gamma[0][2], "the lone embedding joins its voice: {:?}", gamma[0]);
    }
}
