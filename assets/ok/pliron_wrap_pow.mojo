# Defined wrapping for `**` (square-and-multiply over wrapping multiplication,
# the shared native ABI contract shared with the runtime's mjrt_pow helper).
def compute() -> Int:
    var base = 3
    var big = base ** 41
    var identity = base ** 0
    return big + identity

def main():
    print(compute())
