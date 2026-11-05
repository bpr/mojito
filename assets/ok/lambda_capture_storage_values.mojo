# Lambda capture semantics through closure storage: an `{x}` capture reads
# the variable at the call (observing later outer updates), `{mut x}`
# writes through to it, `{var}` snapshots at the lambda's evaluation
# point, and `{}` captures nothing.
# Each closure's value is its storage struct, held in an unannotated local
# or passed straight to an `F: def() -> Int` binder.
def call[F: def() -> Int](f: F) -> Int:
    return f()

def main():
    var limit = 10
    print(call(lambda {limit} -> Int: limit))
    limit = 3
    print(call(lambda {limit} -> Int: limit))

    var pieces: List[Int] = [1]
    var extend = lambda (x: Int) {mut pieces}: pieces.append(x)
    extend(2)
    extend(3)
    print(len(pieces))

    var n = 1
    var snapshot = lambda {var n} -> Int: n
    n = 99
    print(call(snapshot))

    print((lambda (x: Int) {} -> Int: x - 1)(8))
