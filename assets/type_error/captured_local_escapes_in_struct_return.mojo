# expect: escapes storage
# A capturing closure stored in `capturing[_]` struct storage keeps its
# concrete capture origins in the checker's aggregate bookkeeping, so
# returning the struct while a capture roots at a local rejects.
@fieldwise_init
struct Env:
    var callback: def() capturing[_] -> Int

def make() -> Env:
    var n = 1
    def peek() unified {imm n} -> Int:
        return n
    return Env(peek)

def main():
    var env = make()
