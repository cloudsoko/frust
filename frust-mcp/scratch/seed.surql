-- WO-059 MCP probe: identity seed only.
-- Identities are write-closed (WO-008): no REST door creates app_user rows, so
-- the three principals must be seeded directly as root. EVERYTHING else in the
-- fixture (the doctype, its workflow, the rows) is created through the running
-- kernel's REST surface, exactly as a BYO client would — the probe dogfoods it.
--
-- Isolation: a fresh, uniquely-named database on the shared dev surreal (the
-- kernel binary hardcodes endpoint 127.0.0.1:8899, so a separate surreal PROCESS
-- is not reachable without a kernel edit the probe forbids). REMOVE-then-DEFINE
-- guarantees the fixture is clean and never sees the dev store's data.

DEFINE NAMESPACE IF NOT EXISTS frust;
USE NS frust;
REMOVE DATABASE IF EXISTS frustmcp;
DEFINE DATABASE frustmcp;
USE DB frustmcp;

-- Record-auth users. Row-level: a user sees only itself (mirrors setup.surql).
DEFINE TABLE OVERWRITE app_user SCHEMAFULL
    PERMISSIONS
        FOR select WHERE id = $auth.id
        FOR create, update, delete NONE;
DEFINE FIELD OVERWRITE name ON app_user TYPE string;
DEFINE FIELD OVERWRITE role ON app_user TYPE string;
DEFINE FIELD OVERWRITE pass ON app_user TYPE string;
DEFINE INDEX OVERWRITE app_user_name ON app_user FIELDS name UNIQUE;

DEFINE ACCESS OVERWRITE account ON DATABASE TYPE RECORD
    SIGNIN (
        SELECT * FROM app_user
        WHERE name = $name AND crypto::argon2::compare(pass, $pass)
    )
    DURATION FOR TOKEN 1h, FOR SESSION 12h;

CREATE app_user:manager SET name = 'manager', role = 'manager', pass = crypto::argon2::generate('pw-manager');
CREATE app_user:clerk1  SET name = 'clerk1',  role = 'clerk',   pass = crypto::argon2::generate('pw-clerk1');
CREATE app_user:clerk2  SET name = 'clerk2',  role = 'clerk',   pass = crypto::argon2::generate('pw-clerk2');
