def compute() -> Float64:
    var x = 0.1
    var y = 0.2
    var z = x + y
    z = z * 10.0
    z = z - 0.5
    return z / 4.0

def main():
    print(compute())
