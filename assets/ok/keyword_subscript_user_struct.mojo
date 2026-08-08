@fieldwise_init
struct Buf:
    var data: List[Int]

    def __getitem__(self, *, byte: Int) -> Int:
        return self.data[byte]

    def __getitem__(self, row: Int, *, offset: Int) -> Int:
        return self.data[row + offset]

def main():
    var buf = Buf([10, 20, 30])
    print(buf[byte=1])
    print(buf[0, offset=2])
