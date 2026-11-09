root = r'C:/Users/Filip/Documents/Accordion Solitaire/crates/cli/src/'
def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8', newline='').read()
    crlf = '\r\n' in s
    s = s.replace('\r\n', '\n')
    for a, b in pairs:
        assert s.count(a) == 1, (path, a[:80], s.count(a))
        s = s.replace(a, b)
    if crlf:
        s = s.replace('\n', '\r\n')
    open(p, 'w', encoding='utf-8', newline='').write(s)

edit('runner.rs', [
    ('''use rayon::slice::ParallelSliceMut;''', '''use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;'''),
    ('''pub struct Options {''', '''/// The deals of a run, by position: a range of seeds (nothing to hold, for runs of billions) or a list.
#[derive(Clone, Copy)]
pub enum Seeds<'a> {
    Range { from: u64, count: usize },
    List(&'a [u64]),
}

impl Seeds<'_> {
    pub fn count(&self) -> usize {
        match *self {
            Seeds::Range { count, .. } => count,
            Seeds::List(seeds) => seeds.len(),
        }
    }

    /// The seed at position `i`.
    pub fn seed(&self, i: usize) -> u64 {
        match *self {
            Seeds::Range { from, .. } => from + i as u64,
            Seeds::List(seeds) => seeds[i],
        }
    }
}

pub struct Options {'''),
    ('''    /// The solve record as one JSON line, when asked for.
    pub record: Option<String>,
    /// [`fingerprint`] of the deal.
    pub fingerprint: u64,
}''', '''    /// The solve record as one JSON line, when asked for.
    pub record: Option<String>,
}'''),
    ('''pub fn run(
    seeds: &[u64],''', '''pub fn run(
    seeds: Seeds,'''),
    ('''                            let seed = seeds[job.index];''', '''                            let seed = seeds.seed(job.index);'''),
    ('''                            if index < seeds.len() {
                                let solved = solve_one(index, seeds[index], config, &mut search, opts.records);''',
     '''                            if index < seeds.count() {
                                let solved = solve_one(index, seeds.seed(index), config, &mut search, opts.records);'''),
    ('''fn feed(
    gpu: &Gpu,
    seeds: &[u64],''', '''fn feed(
    gpu: &Gpu,
    seeds: Seeds,'''),
    ('''        |&index: &usize| deal_codes(&Deal::from_seed(seeds[index])),''', '''        |&index: &usize| deal_codes(&Deal::from_seed(seeds.seed(index))),'''),
    ('''            (first < seeds.len()).then(|| (first..(first + batch).min(seeds.len())).collect())''',
     '''            (first < seeds.count()).then(|| (first..(first + batch).min(seeds.count())).collect())'''),
    ('''        return Solved { index, seed, report, us, verified, record: None, fingerprint: fingerprint(deal) };''',
     '''        return Solved { index, seed, report, us, verified, record: None };'''),
    ('''    Solved { index, seed, report, us, verified, record, fingerprint: fingerprint(deal) }''',
     '''    Solved { index, seed, report, us, verified, record }'''),
    ('''/// The pairs of seeds (from `seeds`, with `fingerprints` in the same order) whose deals are the same game, even with
/// suits and ranks renamed. Deals whose fingerprints agree are compared card by card.
pub fn same_games(seeds: &[u64], fingerprints: &[u64]) -> Vec<(u64, u64)> {
    let mut sorted = fingerprints.to_vec();
    sorted.par_sort_unstable();
    let repeated: HashSet<u64> = sorted.windows(2).filter(|w| w[0] == w[1]).map(|w| w[0]).collect();
    let mut groups: HashMap<u64, Vec<u64>> = HashMap::new();
    for (&seed, f) in seeds.iter().zip(fingerprints) {
        if repeated.contains(f) {
            groups.entry(*f).or_default().push(seed);
        }
    }''', '''/// The pairs of seeds whose deals are the same game, even with suits and ranks renamed: deals whose fingerprints
/// (`print`: [`fingerprint`], but for tests) agree, compared card by card. Holds one fingerprint per deal, sorted in
/// place (8 GB for a billion deals); the few seeds behind a repeated one are found by fingerprinting again.
pub fn same_games(seeds: Seeds, print: impl Fn(u64) -> u64 + Sync) -> Vec<(u64, u64)> {
    let mut prints: Vec<u64> = (0..seeds.count()).into_par_iter().map(|i| print(seeds.seed(i))).collect();
    prints.par_sort_unstable();
    let repeated: HashSet<u64> = prints.windows(2).filter(|w| w[0] == w[1]).map(|w| w[0]).collect();
    drop(prints);
    if repeated.is_empty() {
        return Vec::new();
    }
    let members: Vec<(u64, u64)> = (0..seeds.count())
        .into_par_iter()
        .filter_map(|i| {
            let seed = seeds.seed(i);
            let f = print(seed);
            repeated.contains(&f).then_some((f, seed))
        })
        .collect();
    let mut groups: HashMap<u64, Vec<u64>> = HashMap::new();
    for (f, seed) in members {
        groups.entry(f).or_default().push(seed);
    }'''),
    ('''        let totals = run(
            &seeds,''', '''        let totals = run(
            Seeds::List(&seeds),'''),
    ('''                assert!(s.record.is_some());
                assert_eq!(s.fingerprint, fingerprint(&Deal::from_seed(s.seed)));''', '''                assert!(s.record.is_some());'''),
    ('''        let seeds = [1, 2, 3, 4];
        let mut prints: Vec<u64> = seeds.iter().map(|&s| fingerprint(&Deal::from_seed(s))).collect();
        assert!(same_games(&seeds, &prints).is_empty());
        // Seed 4 given seed 2's fingerprint: compared card by card, they still differ.
        prints[3] = prints[1];
        assert!(same_games(&seeds, &prints).is_empty());
        // The same seed twice is the same game.
        let seeds = [1, 2, 1];
        let prints: Vec<u64> = seeds.iter().map(|&s| fingerprint(&Deal::from_seed(s))).collect();
        assert_eq!(same_games(&seeds, &prints), vec![(1, 1)]);''', '''        let print = |s: u64| fingerprint(&Deal::from_seed(s));
        let seeds = [1, 2, 3, 4];
        assert!(same_games(Seeds::List(&seeds), print).is_empty());
        // Seed 4 given seed 2's fingerprint: compared card by card, they still differ.
        assert!(same_games(Seeds::List(&seeds), |s| print(if s == 4 { 2 } else { s })).is_empty());
        // The same seed twice is the same game.
        assert_eq!(same_games(Seeds::List(&[1, 2, 1]), print), vec![(1, 1)]);
        assert!(same_games(Seeds::Range { from: 0, count: 10_000 }, print).is_empty());'''),
])

edit('batch.rs', [
    ('''use crate::runner::{self, Options, percentile, wilson};''', '''use crate::runner::{self, Options, Seeds, percentile, wilson};'''),
    ('''    let seeds: Vec<u64> = (args.from..args.from + args.count).collect();''',
     '''    let seeds = Seeds::Range { from: args.from, count: args.count as usize };'''),
    ('''    let mut meter = Meter::new(seeds.len(),''', '''    let mut meter = Meter::new(seeds.count(),'''),
    ('''    let mut nodes = Vec::with_capacity(seeds.len());
    let mut micros = Vec::with_capacity(seeds.len());''', '''    // Grown as needed: only the CPU's deals have them (a fifth or so), and a billion deals' worth is 16 GB.
    let mut nodes = Vec::new();
    let mut micros = Vec::new();'''),
    ('''    let mut fingerprints = vec![0u64; seeds.len()];
''', ''),
    ('''        &seeds,
        &config,''', '''        seeds,
        &config,'''),
    ('''            fingerprints[s.index] = s.fingerprint;
''', ''),
    ('''    let same = runner::same_games(&seeds, &fingerprints);
    drop(fingerprints);''', '''    let same = runner::same_games(seeds, |s| runner::fingerprint(&Deal::from_seed(s)));'''),
])

edit('experiment.rs', [
    ('''runner::run(&seeds, &v.solver, &opts, |_| Ok(()), |_| {})?;''', '''runner::run(Seeds::List(&seeds), &v.solver, &opts, |_| Ok(()), |_| {})?;'''),
    ('''                &seeds,
''', '''                Seeds::List(&seeds),
'''),
])
print('ok')
