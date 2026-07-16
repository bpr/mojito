def classify(value: Int) -> Int:
    return value + 1

def classify(value: Float64) -> Float64:
    return value + 0.5

def main():
    print(classify(4))
    print(classify(2.0))
