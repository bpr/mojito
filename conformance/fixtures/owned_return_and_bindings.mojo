def increment(value: Int) -> Int:
    var result: Int = value + 1
    return result

def main():
    var inferred = increment(4)
    var typed: Int = increment(inferred)
    print(inferred, typed)
