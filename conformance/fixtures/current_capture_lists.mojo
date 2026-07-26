def captured_total() -> Int:
    var total = 0
    var base = 38
    var forwarded = 0
    var snapshot = 1

    def accumulate() {
        mut total, imm base, ref forwarded, var snapshot
    }:
        forwarded += 1
        total = base + forwarded + snapshot

    # `var snapshot` copies when the closure is declared, not when it is called.
    snapshot = 100
    # Reference captures keep a live handle and observe intervening outer writes.
    base = 40
    accumulate()
    return total


def main():
    print(captured_total())
