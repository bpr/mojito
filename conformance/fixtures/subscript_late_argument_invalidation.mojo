def mutate(mut values: List[Int]) -> Int:
    values.append(3)
    return 0


@fieldwise_init
struct Reader:
    var marker: Int

    def __getitem__[origin: Origin[mut=False]](
        self, ref[origin] first: Int, second: Int
    ) -> Int:
        return first + second


def main():
    var values = [10, 20]
    ref first = values[0]
    var reader = Reader(0)
    print(reader[first, mutate(values)])
