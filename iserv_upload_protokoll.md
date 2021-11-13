# IServ File Protocol

## Check
Route: file/check \
Method: POST \
Body: 
```
files[]=<FILENAME>&path=<UPLOAD_DIR>
```
Response:
```
[<EXSISTING_FILE_NAME>]
```

## Upload
Route: file/upload \
Method: POST \
BOUNDARY = `---------------------------<RANDOM_30_DIGETS>` \
Content-Type:
`multipart/form-data boundary=<BOUNDARY>`

UPLOAD_TOKEN is in html in element with ID: upload__token \
UUID = gen with function :
```javascript
function uuidv4 () {
    return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (
        (e) => {
            var t = 16 * Math.random() | 0;
            return ('x' === e ? t : 3 & t | 8).toString(16) 
        })
    )
}
```

BODY:
```
<BOUNDARY>
Content-Disposition: form-data; name="dzuuid"

<UUID>
<BOUNDARY>
Content-Disposition: form-data; name="dzchunkindex"

0
<BOUNDARY>
Content-Disposition: form-data; name="dztotalfilesize"

<UPLOAD_FILE_SIZE>
<BOUNDARY>
Content-Disposition: form-data; name="dzchunksize"

2000000
<BOUNDARY>
Content-Disposition: form-data; name="dztotalchunkcount"

1
<BOUNDARY>
Content-Disposition: form-data; name="dzchunkbyteoffset"

0
<BOUNDARY>
Content-Disposition: form-data; name="upload[path]"

<UPLOAD_DIR>
<BOUNDARY>
Content-Disposition: form-data; name="upload[_token]"

<UPLOAD_TOKEN>
<BOUNDARY>
Content-Disposition: form-data; name="file"; filename="<FILE_NAME>"
Content-Type: application/octet-stream

<FILE_CONTENT_BYTES>
<BOUNDARY>--
```

## Delete
ROUTE: file/multiaction
METHOD: POST
Body:

TOKEN = in html id form__token
```
form[files][]=<FILE_ID>&
form[path]=<FILE_DIR>&
form[current_search]=""&
form[confirm]=1&
from[action]=delete&
from[action][confirm]=""&
from[_token]=<TOKEN>
```
Response:
Status code 302

## Files
COUNTER = counts up every request, but starts at the current unix time \
Route: `file/-/Files?<DIR_PATH>_=<COUNTER>` \
Method: GET