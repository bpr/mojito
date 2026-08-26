# Benchmark: scalar integer arithmetic + branch-heavy loops.
# Collatz sequence lengths over a range plus a branchy modular checksum loop.
def collatz_len(n0: Int) -> Int:
    var n: Int = n0
    var steps: Int = 0
    while n != 1:
        if n % 2 == 0:
            n = n // 2
        else:
            n = 3 * n + 1
        steps += 1
    return steps

def main():
    var total_steps: Int = 0
    var max_len: Int = 0
    var max_n: Int = 0
    var n: Int = 1
    while n <= 12000:
        var length: Int = collatz_len(n)
        total_steps += length
        if length > max_len:
            max_len = length
            max_n = n
        n += 1

    var check: Int = 0
    var i: Int = 0
    while i < 900000:
        if i % 3 == 0:
            check += i
        elif i % 5 == 0:
            check += i // 2
        else:
            check += 1
        check = check % 1000000007
        i += 1

    print("collatz_total:", total_steps)
    print("collatz_max:", max_len, "at", max_n)
    print("checksum:", check)
