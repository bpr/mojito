# expect: use of uninitialized value 'p'
@fieldwise_init
struct T:
    var x: Int

    def get(self) -> Int:
        return self.x

def main():
    var p: T = T(1)
    var q: T = p^
    print(p.get())
