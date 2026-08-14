# expect: @implicit requires a non-raising single-argument
# The loosened @implicit gate admits exactly one `ref [origin]` parameter;
# two reference parameters stay rejected.
struct Wrap:
    var v: Int

    @implicit
    def __init__(out self, ref a: Int, ref b: Int):
        self.v = a + b

def main():
    print("no")
