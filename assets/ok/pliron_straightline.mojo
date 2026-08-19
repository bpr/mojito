def compute() -> Int:
    var a = 40 + 2 * 3 - -5
    var b = (a << 2) >> 1
    var c = (b & 96) | (5 ^ 3)
    var d = (a << 65) + (b >> 64)
    return a * 1000000 + b * 1000 + c + d


def main():
    print(compute())
