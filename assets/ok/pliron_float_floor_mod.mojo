def compute() -> Float64:
    var total = 0.0
    total = total + 7.5 // 2.0
    total = total + (-7.5) // 2.0
    total = total + 7.5 % 2.0
    total = total + (-7.5) % 2.0
    total = total + 7.5 % (-2.0)
    var zero = 0.0
    var inf_part = 1.0 // zero
    if inf_part > 1000000.0:
        total = total + 1.0
    var nan_part = 0.0 % zero
    if nan_part != nan_part:
        total = total + 2.0
    return total

def main():
    print(compute())
