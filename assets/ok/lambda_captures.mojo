# Lambda capture semantics: an omitted capture list imm-captures free variables
# (observing later outer updates), `{mut}` writes through, `{var}` snapshots at
# the lambda's evaluation point, and `{}` captures nothing.
def call(f: def() capturing[_] -> Int) -> Int:
    return f()

def main():
    var limit = 10
    print(call(lambda -> Int: limit))
    limit = 3
    print(call(lambda -> Int: limit))

    var pieces: List[Int] = [1]
    var extend: def(x: Int) capturing[_] = lambda (x: Int) {mut pieces}: pieces.append(x)
    extend(2)
    extend(3)
    print(len(pieces))

    var n = 1
    var snapshot: def() capturing[_] -> Int = lambda {var n} -> Int: n
    n = 99
    print(call(snapshot))

    print((lambda (x: Int) {} -> Int: x - 1)(8))
