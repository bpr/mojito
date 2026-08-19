def pack(q: Int, r: Int) -> Int:
    return q * 100 + r


def compute() -> Int:
    var a = pack(7 // 2, 7 % 3)
    var b = pack(-7 // 2, -7 % 3)
    var c = pack(7 // -2, 7 % -3)
    var d = pack(-7 // -2, -7 % -3)
    return a * 1000000000 + b * 1000000 + c * 1000 + d


def main():
    print(compute())
