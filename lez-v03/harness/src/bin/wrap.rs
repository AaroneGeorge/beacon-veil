//! `wrap <user_elf> <out.bin>`: wraps a guest user ELF with the RISC Zero v1compat kernel into the `ProgramBinary`
//! (`.bin`) that the program loader deploys, as risc0-build does, and prints the image id (hex, as `ProgramId` words
//! little-endian) and the program id words. Same logic as the root workspace's `tools/guest_ids`.

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() == 3, "usage: wrap <user_elf> <out.bin>");
    let elf = std::fs::read(&a[1])?;
    let bin = risc0_binfmt::ProgramBinary::new(&elf, risc0_zkos_v1compat::V1COMPAT_ELF).encode();
    std::fs::write(&a[2], &bin)?;
    let id = risc0_binfmt::compute_image_id(&bin).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "{}: user ELF {} bytes, bin {} bytes, image id {id}",
        a[2],
        elf.len(),
        bin.len()
    );
    Ok(())
}
