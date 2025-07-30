#[cfg(not(parcoll_loom))]
mod general;

#[cfg(parcoll_loom)]
mod loom;
