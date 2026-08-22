//! Open a store left behind by a crash and report what survived.
use supdb::Reader;
fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).expect("path");
    let r = Reader::open(std::path::Path::new(&path))?;
    let mut total = 0u64;
    let mut min = usize::MAX;
    let mut max = 0usize;
    for k in 0..3000usize {
        let mut key = vec![b'k'; 10];
        let mut v = k;
        for i in (1..10).rev() {
            key[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        let mut n = 0usize;
        r.read_all(&key, |_| n += 1)?;
        total += n as u64;
        min = min.min(n);
        max = max.max(n);
    }
    println!(
        "recovered {} keys | values per key: min {} max {} | total {}",
        r.keys(),
        min,
        max,
        total
    );
    Ok(())
}
