# expect: no overload matches
@fieldwise_init
struct Buf:
    var data: List[Int]

    def __getitem__(self, *, byte: Int) -> Int:
        return self.data[byte]

def main():
    var buf = Buf([10, 20, 30])
    print(buf[offset=1])
