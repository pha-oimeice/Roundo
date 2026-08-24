#[derive(roundo_proc_macros::UnixCommand)]
#[unix(path = "broken")]
struct BrokenUnix {
    values: Vec<String>,
    required: String,
}

fn main() {}
