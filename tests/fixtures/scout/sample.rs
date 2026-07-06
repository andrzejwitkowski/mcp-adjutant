// comment mentions invoke() — should be ignored by AST
fn main() {
    invoke();
    let s = "invoke()";
    helper.invoke();
}

fn invoke() {}
