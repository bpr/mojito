# expect: no overload matches
# Growing a `List[ref T]` by appending a reference is not offered: the only
# handle installs happen at construction, where origin binding ties them to
# caller-owned storage.
@fieldwise_init
struct RefList[origin: Origin[mut=True]]:
    var values: List[ref[origin] Int]
    def grow(mut self):
        var local = 9
        ref alias = local
        self.values.append(alias)

def main():
    print(1)
