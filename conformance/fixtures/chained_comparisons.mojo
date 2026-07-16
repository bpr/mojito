def in_range(value: Int, limit: Int) -> Int:
    if 0 <= value < limit:
        return 1
    return 0

def main():
    print(in_range(3, 5))
    print(in_range(7, 5))
    print(in_range(-1, 5))
