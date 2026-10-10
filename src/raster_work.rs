//! Joined, disjoint row work for large pixel buffers; small rasters stay serial.

pub(crate) fn process_rows(
    data: &mut [u8],
    row_bytes: usize,
    pixels: usize,
    process: impl Fn(&mut [u8], usize) + Sync,
) {
    debug_assert!(row_bytes != 0 && data.len().is_multiple_of(row_bytes));
    let rows = data.len() / row_bytes;
    let workers = if pixels >= 256 * 1024 {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(4).min(rows.max(1)))
    } else {
        1
    };
    if workers == 1 {
        process(data, 0);
    } else {
        let rows_per_job = rows.div_ceil(workers);
        std::thread::scope(|scope| {
            for (job, data) in data.chunks_mut(rows_per_job * row_bytes).enumerate() {
                let process = &process;
                scope.spawn(move || process(data, job * rows_per_job));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::process_rows;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn small_and_empty_buffers_stay_serial() {
        for mut data in [vec![], vec![0; 64]] {
            let calls = AtomicUsize::new(0);
            process_rows(&mut data, 16, 16, |_, first| {
                assert_eq!(first, 0);
                calls.fetch_add(1, Ordering::Relaxed);
            });
            assert_eq!(calls.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn large_uneven_row_ranges_finish_exactly_once_before_returning() {
        let (width, height) = (641, 431);
        let row_bytes = width * 4;
        let mut data = vec![0; row_bytes * height];
        let calls = AtomicUsize::new(0);
        let seen: Vec<_> = (0..height).map(|_| AtomicUsize::new(0)).collect();
        process_rows(&mut data, row_bytes, width * height, |chunk, first| {
            calls.fetch_add(1, Ordering::Relaxed);
            for (row, bytes) in chunk.chunks_exact_mut(row_bytes).enumerate() {
                let row = first + row;
                seen[row].fetch_add(1, Ordering::Relaxed);
                bytes.fill((row % 251) as u8);
            }
        });
        assert!((1..=4).contains(&calls.load(Ordering::Relaxed)));
        for (row, bytes) in data.chunks_exact(row_bytes).enumerate() {
            assert_eq!(seen[row].load(Ordering::Relaxed), 1);
            assert!(bytes.iter().all(|&byte| byte == (row % 251) as u8));
        }
    }
}
