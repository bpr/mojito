# A consuming overload must not replay the loan effect recorded by its
# same-name borrowing-view sibling.
def append_to_fresh(var other: List[Int]) -> List[Int]:
    var result: List[Int] = [1, 2]
    result.extend(other^)
    return result^

def main():
    var moved: List[Int] = [3, 4]
    var combined = append_to_fresh(moved^)
    print(len(combined), combined[3])

    # The borrowing overload still records and executes its conversion.
    var source: List[Int] = [5, 6]
    var target: List[Int] = [0]
    target.extend(source)
    print(len(target), target[2], len(source))
# stdout: 4 4
# stdout: 3 6 2
