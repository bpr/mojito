# expect: expression must be mutable in collection literal
# A subscript target must yield a mutable reference; a setter alone does not make it one.
struct Sink(Copyable):
    var a: Int
    def __init__(out self):
        self.a = 0
    def __getitem__(self, i: Int) -> Int:
        return self.a
    def __setitem__(mut self, i: Int, v: Int):
        self.a = v
def main() raises:
    var s = Sink()
    var values: List[Int] = [1, 2]
    [s[0], s[1]] = values
    print(s.a)
