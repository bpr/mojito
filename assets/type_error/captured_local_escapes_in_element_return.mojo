# expect: escapes storage
# The same retention covers collection elements: a list of capturing
# callables carrying a local-rooted capture cannot be returned.
def make() -> List[def() capturing[_] -> Int]:
    var n = 1
    def peek() unified {imm n} -> Int:
        return n
    var fns: List[def() capturing[_] -> Int] = [peek]
    return fns^

def main():
    var fns = make()
