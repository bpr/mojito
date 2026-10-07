# expect: is missing method 'write_string'
@fieldwise_init
struct BrokenWriter(Writer):
    var buffer: String
