# Coverage is nonfatal: a non-exhaustive `Bool` match, a duplicate literal,
# a case after a catch-all, an uncovered `(Bool, Bool)` cell, and a guarded
# arm that adds no coverage each warn, and the program runs; an exhaustive
# `Bool` match whose cases return satisfies return analysis, and a guard
# may read the subject's place.
def k(x: Bool) -> Int:
    match x:
        case True:
            return 10
        case False:
            return 20


def main():
    var b = True
    match b:
        case True:
            print("t")
    match 5:
        case 0:
            print("zero")
        case 0:
            print("again")
        case _:
            print("other")
        case 7:
            print("seven")
    match (b, False):
        case (True, True):
            print("tt")
        case (False, _):
            print("f_")
    match b:
        case True if k(b) == 10:
            print("guarded")
        case False:
            print("f")
    print(k(True), k(False))
