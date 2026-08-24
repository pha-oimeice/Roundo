#[derive(roundo_proc_macros::UnixCommand)]
#[unix(path = "broken")]
struct OptionalBeforeRequired {
    optional: Option<String>,
    required: String,
}

fn main() {}
