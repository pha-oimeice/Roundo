#[derive(roundo_proc_macros::UnixCommand)]
#[unix(path = "broken")]
struct TupleUnix(String);

fn main() {}
