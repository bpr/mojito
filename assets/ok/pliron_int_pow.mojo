def compute() -> Int:
    var total = 0
    total = total + 2 ** 10
    total = total + (-3) ** 3
    total = total + 5 ** 0
    total = total + 0 ** 0
    var b = 2
    var e = 30
    total = total + b ** e
    return total

def main():
    print(compute())
