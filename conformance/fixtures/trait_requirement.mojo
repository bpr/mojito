trait Valued:
    def value(self) -> Int: ...

@fieldwise_init
struct Number(Valued):
    var data: Int

    def value(self) -> Int:
        return self.data

def read_value[T: Valued](value: T) -> Int:
    return value.value()

def main():
    print(read_value(Number(7)))
