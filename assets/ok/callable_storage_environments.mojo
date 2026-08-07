# Thin (capture-free) functions store into plain `def(...)` fields and
# elements; a capturing closure stores only into `capturing[...]`-annotated
# storage, which retains its environment contract.
@fieldwise_init
struct Plain:
    var callback: def(Int) -> Int

@fieldwise_init
struct Env:
    var callback: def(Int) capturing[_] -> Int

def double(x: Int) -> Int:
    return x * 2

def main():
    var plain = Plain(double)
    var fns: List[def(Int) -> Int] = [double]
    var n = 1
    def bump(x: Int) unified {imm n} -> Int:
        return x + n
    var env = Env(bump)
    print(len(fns))
