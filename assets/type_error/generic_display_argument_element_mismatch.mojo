# A list display whose elements do not convert to the explicitly solved
# element type of its parameter is rejected, naming the parameter.
# expect: type mismatch for argument 'keys' to 'W.__init__': expected List[String], found list literal
struct W[K: Copyable & Deinitable]:
    var keys: List[Self.K]

    def __init__(out self, var keys: List[Self.K], n: Optional[Int]):
        self.keys = keys^

def main():
    var w = W[String]([1, 2], None)
    print(len(w.keys))
