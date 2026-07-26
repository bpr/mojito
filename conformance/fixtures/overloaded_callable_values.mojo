def transform(value: Int) -> Int:
    return value + 1

def transform(value: String) raises -> Int:
    raise Error("string overload")

def main():
    var integer_transform: def(Int) thin -> Int = transform
    print(integer_transform(41))
    var raising_transform: def(String) raises thin -> Int = transform
    try:
        print(raising_transform("selected by type and effect"))
    except:
        print("caught")
