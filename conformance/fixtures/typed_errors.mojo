@fieldwise_init
struct ValidationError:
    var field: String
    var reason: String

def validate() raises ValidationError -> Int:
    raise ValidationError("name", "empty")

def main():
    try:
        _ = validate()
    except error:
        print(error.field, error.reason)
