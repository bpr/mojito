# expect: type mismatch for variable 'b': expected Bool, found DType
# A `DType` does not convert to `Bool`.
def main():
    var d = DType.int
    var b: Bool = d
    print(b)
