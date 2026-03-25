port module Main exposing (main)

import Browser
import Browser.Navigation as Nav
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Http
import Json.Decode as D
import Json.Encode as E
import Url exposing (Url)



-- ── PORTS ─────────────────────────────────────────────────────────────────────


{-| Inject server-rendered HTML into the wiki content div.
Set via JS port after the DOM is updated (see index.html).
-}
port setPageContent : String -> Cmd msg


{-| Inject preview HTML into the preview div.
-}
port setPreviewContent : String -> Cmd msg


{-| Receive clicks on internal links inside injected HTML.
JS intercepts these and sends the href here so Elm can do SPA navigation.
-}
port linkClicked : (String -> msg) -> Sub msg



-- ── ROUTING ───────────────────────────────────────────────────────────────────


type Route
    = PageView String
    | EditPage String


routeFromUrl : Url -> Route
routeFromUrl url =
    let
        path =
            url.path
    in
    if String.startsWith "/edit/" path then
        EditPage (String.dropLeft 6 path |> ensureOrgExt)

    else
        let
            stripped =
                String.dropLeft 1 path
        in
        PageView
            (if String.isEmpty stripped then
                "index.org"

             else
                ensureOrgExt stripped
            )


pagePathFromRoute : Route -> String
pagePathFromRoute route =
    case route of
        PageView p ->
            p

        EditPage p ->
            p


ensureOrgExt : String -> String
ensureOrgExt s =
    if String.endsWith ".org" s then
        s

    else
        s ++ ".org"



-- ── TYPES ─────────────────────────────────────────────────────────────────────


type alias User =
    { name : String
    , email : String
    }


type alias Page =
    { title : String
    , html : String
    , rawOrg : String
    , pagePath : String
    , exists : Bool
    }


type PageState
    = Loading
    | Loaded Page
    | Failed String


type PreviewState
    = NoPreview
    | PreviewLoading
    | PreviewReady


type alias EditState =
    { content : String
    , subject : String
    , body : String
    , preview : PreviewState
    , saving : Bool
    , error : Maybe String
    }


type alias Model =
    { key : Nav.Key
    , route : Route
    , user : Maybe User
    , pageState : PageState
    , editState : EditState
    }



-- ── MSG ───────────────────────────────────────────────────────────────────────


type Msg
    = UrlRequested Browser.UrlRequest
    | UrlChanged Url
    | LinkClicked String
    | GotMe (Result Http.Error User)
    | GotPage String (Result Http.Error Page)
    | GotPreview (Result Http.Error String)
    | SaveDone (Result Http.Error String)
    | EditContentChanged String
    | EditSubjectChanged String
    | EditBodyChanged String
    | ShowPreview
    | ShowWrite
    | SubmitSave



-- ── INIT ──────────────────────────────────────────────────────────────────────


initEditState : EditState
initEditState =
    { content = ""
    , subject = ""
    , body = ""
    , preview = NoPreview
    , saving = False
    , error = Nothing
    }


init : () -> Url -> Nav.Key -> ( Model, Cmd Msg )
init _ url key =
    let
        route =
            routeFromUrl url

        pagePath =
            pagePathFromRoute route
    in
    ( { key = key
      , route = route
      , user = Nothing
      , pageState = Loading
      , editState = initEditState
      }
    , Cmd.batch
        [ fetchMe
        , fetchPage pagePath
        ]
    )



-- ── UPDATE ────────────────────────────────────────────────────────────────────


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        UrlRequested (Browser.Internal url) ->
            let
                path =
                    url.path
            in
            if String.startsWith "/auth/" path then
                -- Auth routes are handled server-side; force a full page load.
                ( model, Nav.load (Url.toString url) )

            else
                ( model, Nav.pushUrl model.key (Url.toString url) )

        UrlRequested (Browser.External url) ->
            ( model, Nav.load url )

        UrlChanged url ->
            let
                route =
                    routeFromUrl url

                pagePath =
                    pagePathFromRoute route
            in
            ( { model
                | route = route
                , pageState = Loading
                , editState = initEditState
              }
            , fetchPage pagePath
            )

        LinkClicked href ->
            ( model, Nav.pushUrl model.key href )

        GotMe result ->
            case result of
                Ok user ->
                    ( { model | user = Just user }, Cmd.none )

                Err _ ->
                    -- 401 or network error — treat as not logged in.
                    ( { model | user = Nothing }, Cmd.none )

        GotPage pagePath result ->
            -- Discard stale responses for pages we've navigated away from.
            if pagePath /= pagePathFromRoute model.route then
                ( model, Cmd.none )

            else
                case result of
                    Ok page ->
                        let
                            newModel =
                                { model | pageState = Loaded page }
                        in
                        case model.route of
                            PageView _ ->
                                ( newModel, setPageContent page.html )

                            EditPage _ ->
                                ( { newModel
                                    | editState =
                                        { initEditState | content = page.rawOrg }
                                  }
                                , Cmd.none
                                )

                    Err err ->
                        ( { model | pageState = Failed (httpErrorToString err) }
                        , Cmd.none
                        )

        GotPreview result ->
            let
                es =
                    model.editState
            in
            case result of
                Ok html ->
                    ( { model | editState = { es | preview = PreviewReady } }
                    , setPreviewContent html
                    )

                Err err ->
                    ( { model
                        | editState =
                            { es
                                | preview = NoPreview
                                , error =
                                    Just
                                        ("Preview failed: "
                                            ++ httpErrorToString err
                                        )
                            }
                      }
                    , Cmd.none
                    )

        SaveDone result ->
            case result of
                Ok _ ->
                    case model.route of
                        EditPage p ->
                            ( model, Nav.pushUrl model.key ("/" ++ p) )

                        _ ->
                            ( model, Cmd.none )

                Err (Http.BadStatus 401) ->
                    let
                        es =
                            model.editState
                    in
                    ( { model
                        | editState =
                            { es
                                | saving = False
                                , error =
                                    Just
                                        "Session expired — please sign in again."
                            }
                      }
                    , Cmd.none
                    )

                Err err ->
                    let
                        es =
                            model.editState
                    in
                    ( { model
                        | editState =
                            { es
                                | saving = False
                                , error = Just (httpErrorToString err)
                            }
                      }
                    , Cmd.none
                    )

        EditContentChanged s ->
            let
                es =
                    model.editState
            in
            ( { model | editState = { es | content = s } }, Cmd.none )

        EditSubjectChanged s ->
            let
                es =
                    model.editState
            in
            ( { model | editState = { es | subject = s } }, Cmd.none )

        EditBodyChanged s ->
            let
                es =
                    model.editState
            in
            ( { model | editState = { es | body = s } }, Cmd.none )

        ShowPreview ->
            let
                es =
                    model.editState
            in
            ( { model | editState = { es | preview = PreviewLoading } }
            , fetchPreview es.content
            )

        ShowWrite ->
            let
                es =
                    model.editState
            in
            ( { model | editState = { es | preview = NoPreview } }, Cmd.none )

        SubmitSave ->
            case model.route of
                EditPage p ->
                    let
                        es =
                            model.editState
                    in
                    if String.isEmpty (String.trim es.subject) then
                        ( { model
                            | editState =
                                { es | error = Just "Edit summary is required." }
                          }
                        , Cmd.none
                        )

                    else
                        ( { model
                            | editState =
                                { es | saving = True, error = Nothing }
                          }
                        , postSave p es
                        )

                _ ->
                    ( model, Cmd.none )



-- ── HTTP ──────────────────────────────────────────────────────────────────────


fetchMe : Cmd Msg
fetchMe =
    Http.get
        { url = "/api/me"
        , expect = Http.expectJson GotMe decodeUser
        }


fetchPage : String -> Cmd Msg
fetchPage pagePath =
    Http.get
        { url = "/api/page/" ++ pagePath
        , expect = Http.expectJson (GotPage pagePath) decodePage
        }


fetchPreview : String -> Cmd Msg
fetchPreview content =
    Http.post
        { url = "/api/preview"
        , body =
            Http.jsonBody
                (E.object [ ( "content", E.string content ) ])
        , expect = Http.expectString GotPreview
        }


postSave : String -> EditState -> Cmd Msg
postSave pagePath es =
    Http.post
        { url = "/api/save/" ++ pagePath
        , body =
            Http.jsonBody
                (E.object
                    [ ( "content", E.string es.content )
                    , ( "subject", E.string es.subject )
                    , ( "body"
                      , if String.isEmpty es.body then
                            E.null

                        else
                            E.string es.body
                      )
                    ]
                )
        , expect = Http.expectJson SaveDone (D.field "commit" D.string)
        }



-- ── DECODERS ──────────────────────────────────────────────────────────────────


decodeUser : D.Decoder User
decodeUser =
    D.map2 User
        (D.field "name" D.string)
        (D.field "email" D.string)


decodePage : D.Decoder Page
decodePage =
    D.map5 Page
        (D.field "title" D.string)
        (D.field "html" D.string)
        (D.field "rawOrg" D.string)
        (D.field "pagePath" D.string)
        (D.field "exists" D.bool)



-- ── MAIN / VIEW ───────────────────────────────────────────────────────────────


main : Program () Model Msg
main =
    Browser.application
        { init = init
        , view = view
        , update = update
        , subscriptions = subscriptions
        , onUrlRequest = UrlRequested
        , onUrlChange = UrlChanged
        }


subscriptions : Model -> Sub Msg
subscriptions _ =
    linkClicked LinkClicked


view : Model -> Browser.Document Msg
view model =
    case model.route of
        PageView _ ->
            { title = pageTitle model ++ " — org-wiki"
            , body = [ viewLayout model (viewPage model) ]
            }

        EditPage _ ->
            { title = "Editing: " ++ pageTitle model ++ " — org-wiki"
            , body = [ viewLayout model (viewEdit model) ]
            }


pageTitle : Model -> String
pageTitle model =
    case model.pageState of
        Loaded page ->
            if String.isEmpty page.title then
                pagePathFromRoute model.route

            else
                page.title

        _ ->
            "…"


viewLayout : Model -> Html Msg -> Html Msg
viewLayout model content =
    div [ id "layout" ]
        [ viewHeader model
        , Html.main_ [ id "main" ] [ content ]
        ]


viewHeader : Model -> Html Msg
viewHeader model =
    let
        editLink =
            case ( model.route, model.pageState, model.user ) of
                ( PageView p, Loaded page, Just _ ) ->
                    if page.exists then
                        [ a [ href ("/edit/" ++ p) ] [ text "Edit" ] ]

                    else
                        []

                _ ->
                    []

        authLinks =
            case model.user of
                Just u ->
                    [ span [ class "user-name" ] [ text u.name ]
                    , a [ href "/auth/logout" ] [ text "Sign out" ]
                    ]

                Nothing ->
                    [ a [ href (loginUrl model.route) ] [ text "Sign in" ] ]
    in
    header [ id "site-header" ]
        [ a [ href "/", class "site-name" ] [ text "org-wiki" ]
        , nav [] (editLink ++ authLinks)
        ]


loginUrl : Route -> String
loginUrl route =
    "/auth/login?next=" ++ routeToPath route


routeToPath : Route -> String
routeToPath route =
    case route of
        PageView p ->
            "/" ++ p

        EditPage p ->
            "/edit/" ++ p


viewPage : Model -> Html Msg
viewPage model =
    case model.pageState of
        Loading ->
            p [ class "loading" ] [ text "Loading…" ]

        Failed msg ->
            div [ class "flash-error" ] [ text msg ]

        Loaded page ->
            if page.exists then
                -- Content is injected via the setPageContent port after the
                -- DOM is updated.  This div must be present for the JS to target.
                div [ id "wiki-content", class "wiki-prose" ] []

            else
                div [ id "not-found" ]
                    [ h1 [] [ text "Page not found" ]
                    , p [] [ text "This page does not exist yet." ]
                    , case model.user of
                        Just _ ->
                            p []
                                [ a [ href ("/edit/" ++ page.pagePath) ]
                                    [ text "Create it" ]
                                ]

                        Nothing ->
                            p [] [ text "Sign in to create it." ]
                    ]


viewEdit : Model -> Html Msg
viewEdit model =
    let
        es =
            model.editState

        pagePath =
            pagePathFromRoute model.route

        isPreview =
            case es.preview of
                NoPreview ->
                    False

                _ ->
                    True
    in
    case model.user of
        Nothing ->
            div [ id "edit-view" ]
                [ p []
                    [ text "You must "
                    , a [ href (loginUrl model.route) ] [ text "sign in" ]
                    , text " to edit pages."
                    ]
                ]

        Just _ ->
            div [ id "edit-view" ]
                [ h1 [] [ text ("Editing: " ++ pageTitle model) ]
                , viewError es.error
                , div [ class "tab-bar" ]
                    [ button
                        [ classList
                            [ ( "tab", True )
                            , ( "tab-active", not isPreview )
                            ]
                        , onClick ShowWrite
                        , type_ "button"
                        ]
                        [ text "Write" ]
                    , button
                        [ classList
                            [ ( "tab", True )
                            , ( "tab-active", isPreview )
                            ]
                        , onClick ShowPreview
                        , type_ "button"
                        ]
                        [ text "Preview" ]
                    ]
                , if isPreview then
                    div [ id "preview-content", class "wiki-prose" ] []

                  else
                    textarea
                        [ id "editor"
                        , value es.content
                        , onInput EditContentChanged
                        ]
                        []
                , div [ class "edit-meta" ]
                    [ label [ class "edit-label" ]
                        [ span [] [ text "Edit summary" ]
                        , input
                            [ type_ "text"
                            , value es.subject
                            , onInput EditSubjectChanged
                            , placeholder "Brief description of your change"
                            ]
                            []
                        ]
                    , label [ class "edit-label" ]
                        [ span [] [ text "Extended description (optional)" ]
                        , textarea
                            [ class "edit-body"
                            , value es.body
                            , onInput EditBodyChanged
                            ]
                            []
                        ]
                    ]
                , div [ class "edit-actions" ]
                    [ button
                        [ onClick SubmitSave
                        , disabled es.saving
                        , type_ "button"
                        , class "btn-primary"
                        ]
                        [ text
                            (if es.saving then
                                "Saving…"

                             else
                                "Save changes"
                            )
                        ]
                    , a [ href ("/" ++ pagePath), class "cancel-link" ]
                        [ text "Cancel" ]
                    ]
                ]


viewError : Maybe String -> Html Msg
viewError maybeMsg =
    case maybeMsg of
        Just msg ->
            div [ class "flash-error" ] [ text msg ]

        Nothing ->
            text ""



-- ── HELPERS ───────────────────────────────────────────────────────────────────


httpErrorToString : Http.Error -> String
httpErrorToString err =
    case err of
        Http.BadUrl url ->
            "Bad URL: " ++ url

        Http.Timeout ->
            "Request timed out."

        Http.NetworkError ->
            "Network error."

        Http.BadStatus status ->
            "Server error (" ++ String.fromInt status ++ ")."

        Http.BadBody msg ->
            "Unexpected response: " ++ msg
