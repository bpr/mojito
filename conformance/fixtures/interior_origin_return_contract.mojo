@fieldwise_init
struct Bucket:
    var values: List[Int]

    def at(
        ref self, index: Int
    ) -> ref[origin_of(self.values)._get_owned_interior["element"]] Int:
        return self.values[index]


def main():
    var bucket = Bucket([1, 2, 3])
    ref first = bucket.at(0)
    bucket.values.append(4)
    print(first)
