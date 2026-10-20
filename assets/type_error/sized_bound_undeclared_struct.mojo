# expect: does not conform to trait 'Sized'
@fieldwise_init
struct Shelf:
    var n: Int

    def __len__(self) -> Int:
        return self.n

def size_of[T: Sized](x: T) -> Int:
    return len(x)

def main():
    print(size_of(Shelf(3)))
