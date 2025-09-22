
source config/.env
export DATABASE_URL=$DATABASE_URL
echo "🔄 All in one script for database with at DATABASE_URL = $DATABASE_URL"

#!/bin/bash
echo "🔄 Resetting database with at DATABASE_URL = $DATABASE_URL"
npx prisma migrate reset --force --schema=ops/prisma/schema.prisma # ! Very destructive, will drop all data

#!/bin/bash
echo "🔄 Pushing database with at DATABASE_URL = $DATABASE_URL"
npx prisma db push --schema=ops/prisma/schema.prisma --force-reset
npx prisma generate --schema=ops/prisma/schema.prisma

#!/bin/bash
echo "🔄 Generating sea-orm entities with at DATABASE_URL = $DATABASE_URL"
sea-orm-cli generate entity \
    -u "$DATABASE_URL" \
    -o src/shd/data/entity \
    --with-serde=both 

