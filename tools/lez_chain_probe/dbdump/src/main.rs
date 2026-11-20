//! Usage: lez_dbdump DB_DIR OUT_DIR
//!
//! Opens DB_DIR (a copy of a sequencer's `rocksdb-*` directory) read-only and writes every block to
//! OUT_DIR/<id>.borsh. The sequencer store of LEZ v0.4.0-rc3 (`lez/sequencer/actors/storage/src/actor/entities.rs`)
//! keeps blocks in column family `Block`, key = the block id as a big-endian u64 (`BigEndian<BlockId>`), value =
//! borsh(Block). Entries whose key does not end in the value's own id are skipped and counted.

use std::{fs, path::Path};

use rocksdb::{DB, IteratorMode, Options};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.len() == 3, "usage: lez_dbdump DB_DIR OUT_DIR");
    let (db_dir, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let cfs = DB::list_cf(&Options::default(), db_dir).expect("list column families");
    let db = DB::open_cf_for_read_only(&Options::default(), db_dir, &cfs, false).expect("open read-only");
    let cf = db.cf_handle("Block").expect("column family Block");
    fs::create_dir_all(out).expect("create out dir");
    let (mut n, mut skipped, mut lo, mut hi) = (0_u64, 0_u64, u64::MAX, 0_u64);
    for item in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = item.expect("read");
        if key.len() < 8 || value.len() < 8 {
            skipped += 1;
            continue;
        }
        let id = u64::from_be_bytes(key[key.len() - 8..].try_into().expect("8 bytes"));
        if value[..8] != id.to_le_bytes() {
            skipped += 1;
            continue;
        }
        fs::write(out.join(format!("{id}.borsh")), &value).expect("write");
        n += 1;
        lo = lo.min(id);
        hi = hi.max(id);
    }
    println!("{{\"blocks\":{n},\"skipped\":{skipped},\"first\":{lo},\"last\":{hi},\"column_families\":{cfs:?}}}");
}
