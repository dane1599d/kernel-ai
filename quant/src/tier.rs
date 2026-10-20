//! The kernel tiers, chosen at runtime from what the CPU has and the OS saves.

/// A set of kernels this CPU can run. Only [`Tier::BASELINE`] and the tiers
/// [`Tier::all`] detects exist, so holding a tier proves the CPU runs it. Every
/// tier gives the same bits as the baseline, for every format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tier(pub(crate) Kind);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Baseline,
    /// AVX2 and FMA, with the OS saving ymm state.
    #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
    Avx2,
    /// AVX-512 F, BW and VL on top of AVX2, with the OS saving zmm and opmask
    /// state.
    #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
    Avx512,
    /// [`Kind::Avx512`] plus AVX-512 VNNI.
    #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
    Avx512Vnni,
}

impl Tier {
    /// Plain Rust, for any CPU.
    pub const BASELINE: Self = Self(Kind::Baseline);

    /// The fastest tier this CPU runs.
    pub fn best() -> Self {
        Self::all().last().unwrap_or(Self::BASELINE)
    }

    /// AVX2, FMA and F16C, when the CPU has them and the OS saves ymm state.
    /// Targets without SSE (the soft-float kernel target) never get it.
    #[cfg(test)]
    pub(crate) fn avx2() -> Option<Self> {
        Self::detected()[1]
    }

    /// Every tier this CPU runs, the baseline first and the fastest last.
    pub fn all() -> impl Iterator<Item = Self> {
        Self::detected().into_iter().flatten()
    }

    /// The baseline, then AVX2, AVX-512 and AVX-512 VNNI where they run.
    fn detected() -> [Option<Self>; 4] {
        let [avx2, avx512, vnni] = kinds(&cpufeat::Features::detect());
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        return [
            Some(Self::BASELINE),
            avx2.then_some(Self(Kind::Avx2)),
            avx512.then_some(Self(Kind::Avx512)),
            vnni.then_some(Self(Kind::Avx512Vnni)),
        ];
        // No SIMD kernel is compiled for a target without SSE.
        #[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
        {
            let _ = (avx2, avx512, vnni);
            [Some(Self::BASELINE), None, None, None]
        }
    }

    pub fn name(self) -> &'static str {
        match self.0 {
            Kind::Baseline => "baseline",
            #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
            Kind::Avx2 => "avx2+fma",
            #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
            Kind::Avx512 => "avx512",
            #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
            Kind::Avx512Vnni => "avx512+vnni",
        }
    }
}

/// Which of the AVX2, AVX-512 and AVX-512 VNNI tiers `cpu` runs. Each needs the
/// one before it. AVX2 also needs FMA and F16C (the kernels widen f16 scales with
/// it) and the OS saving ymm state; AVX-512 needs F, BW and VL and the OS saving
/// zmm and opmask state.
pub(crate) fn kinds(cpu: &cpufeat::Features) -> [bool; 3] {
    let avx2 = cpu.avx2_fma() && cpu.f16c;
    let avx512 = avx2 && cpu.avx512() && cpu.avx512vl;
    let vnni = avx512 && cpu.avx512vnni;
    [avx2, avx512, vnni]
}
