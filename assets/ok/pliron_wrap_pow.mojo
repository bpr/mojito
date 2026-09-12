# Defined wrapping for `**` (square-and-multiply over wrapping multiplication,
# the shared native ABI contract, run by the bundled `_pow_int` body).
def compute() -> Int:
    var base = 3
    var big = base ** 41
    var identity = base ** 0
    return big + identity

def main():
    print(compute())
